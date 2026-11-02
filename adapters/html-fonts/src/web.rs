//! Browser-managed persistent storage is the web equivalent of the native disk
//! cache. In-flight sharing avoids duplicate fetches before CacheStorage fills.
use crate::{MAX_FONT_BYTES, decode};
use futures_util::FutureExt;
use futures_util::future::{LocalBoxFuture, Shared};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Cache, Response, ResponseInit};

type Pending = Shared<LocalBoxFuture<'static, Result<Arc<Vec<u8>>, String>>>;
thread_local! {
    static REQUESTS: RefCell<HashMap<String, Pending>> = RefCell::default();
}

pub(crate) async fn load(url: &str) -> Result<Vec<u8>, String> {
    let pending = REQUESTS.with(|requests| {
        requests
            .borrow_mut()
            .entry(url.to_owned())
            .or_insert_with(|| {
                let url = url.to_owned();
                async move { fetch(&url).await.map(Arc::new) }
                    .boxed_local()
                    .shared()
            })
            .clone()
    });
    let result = pending.await;
    if result.is_err() {
        REQUESTS.with(|requests| {
            requests.borrow_mut().remove(url);
        });
    }
    result.map(|bytes| bytes.as_ref().clone())
}

async fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let global = js_sys::global();
    let caches = js_sys::Reflect::get(&global, &JsValue::from_str("caches"))
        .ok()
        .and_then(|value| value.dyn_into::<web_sys::CacheStorage>().ok());
    let cache = if let Some(caches) = caches {
        JsFuture::from(caches.open("html-engine-fonts-v1"))
            .await
            .ok()
            .and_then(|value| value.dyn_into::<Cache>().ok())
    } else {
        None
    };
    if let Some(cache) = &cache
        && let Ok(value) = JsFuture::from(cache.match_with_str(url)).await
        && let Ok(response) = value.dyn_into::<Response>()
        && let Ok(bytes) = response_bytes(response).await
        && let Ok(bytes) = decode(&bytes)
    {
        return Ok(bytes);
    }
    let fetch: js_sys::Function = js_sys::Reflect::get(&global, &JsValue::from_str("fetch"))
        .map_err(js_error)?
        .dyn_into()
        .map_err(js_error)?;
    let promise: js_sys::Promise = fetch
        .call1(&global, &JsValue::from_str(url))
        .map_err(js_error)?
        .dyn_into()
        .map_err(js_error)?;
    let response: Response = JsFuture::from(promise)
        .await
        .map_err(js_error)?
        .dyn_into()
        .map_err(js_error)?;
    if !response.ok() {
        return Err(format!("font request returned HTTP {}", response.status()));
    }
    let bytes = decode(&response_bytes(response).await?)?;
    if let Some(cache) = cache {
        let init = ResponseInit::new();
        init.set_status(200);
        let body = js_sys::Uint8Array::from(bytes.as_slice());
        let response =
            Response::new_with_opt_js_u8_array_and_init(Some(&body), &init).map_err(js_error)?;
        // Fonts still work in private-browsing/quota-limited sessions where
        // persistent browser storage is unavailable.
        let _ = JsFuture::from(cache.put_with_str(url, &response)).await;
    }
    Ok(bytes)
}

async fn response_bytes(response: Response) -> Result<Vec<u8>, String> {
    let buffer = JsFuture::from(response.array_buffer().map_err(js_error)?)
        .await
        .map_err(js_error)?;
    let array = js_sys::Uint8Array::new(&buffer);
    if array.length() as usize > MAX_FONT_BYTES {
        return Err("font exceeds the size limit".into());
    }
    Ok(array.to_vec())
}

fn js_error(error: JsValue) -> String {
    format!("font cache/fetch failed: {error:?}")
}
