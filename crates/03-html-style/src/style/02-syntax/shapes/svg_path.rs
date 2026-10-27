//! Strict SVG path-data grammar for CSS path() strings. No geometry is built.

pub(super) fn is_valid(source: &str) -> bool {
    let mut path = PathData {
        bytes: source.as_bytes(),
        position: 0,
    };
    path.whitespace();
    if !matches!(path.peek(), Some(b'M' | b'm')) {
        return false;
    }
    while let Some(command) = path.peek() {
        path.position += 1;
        let command = command.to_ascii_lowercase();
        let count = match command {
            b'm' | b'l' | b't' => 2,
            b'h' | b'v' => 1,
            b'c' => 6,
            b's' | b'q' => 4,
            b'a' => 7,
            b'z' => {
                path.whitespace();
                continue;
            }
            _ => return false,
        };
        path.whitespace();
        loop {
            for index in 0..count {
                if index != 0 {
                    path.separator();
                }
                let valid = if command == b'a' && matches!(index, 3 | 4) {
                    path.flag()
                } else {
                    path.number(command == b'a' && index < 2)
                };
                if !valid {
                    return false;
                }
            }
            path.whitespace();
            if path.peek() == Some(b',') {
                path.position += 1;
                path.whitespace();
                // A comma commits to another parameter group. It cannot
                // precede a command, another comma, or the end of the string.
            } else if path.peek().is_none_or(|byte| byte.is_ascii_alphabetic()) {
                break;
            }
        }
    }
    true
}

struct PathData<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl PathData<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.position += 1;
        }
    }

    fn separator(&mut self) {
        self.whitespace();
        if self.peek() == Some(b',') {
            self.position += 1;
            self.whitespace();
        }
    }

    fn flag(&mut self) -> bool {
        if matches!(self.peek(), Some(b'0' | b'1')) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.position;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.position += 1;
        }
        self.position - start
    }

    fn number(&mut self, non_negative: bool) -> bool {
        if !non_negative && matches!(self.peek(), Some(b'+' | b'-')) {
            self.position += 1;
        }
        let mut digits = self.digits();
        if self.peek() == Some(b'.') {
            self.position += 1;
            digits += self.digits();
        }
        if digits == 0 {
            return false;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            if self.digits() == 0 {
                return false;
            }
        }
        true
    }
}
