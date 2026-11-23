# Coding preferences

- In Rust, prefer `use` imports for repeated types, traits, and functions rather than repeatedly writing fully qualified `::` paths inline. Keep qualified paths where they clarify ambiguous names or platform-specific code, and follow the surrounding code style.
- Prefer a single pass when practical, and produce paint output in final paint order during traversal.
- Access struct fields directly instead of introducing local aliases solely to shorten field access.

# Tests

- Only run tests or create tests when explicitly asked by the user. Do not run or create tests automatically as part of implementation, review, or validation.

# Compilation

- Do not run compilation commands, including `cargo check`, `cargo build`, or direct `rustc` invocations, unless the user explicitly asks. Requests to implement, fix, optimize, review, or validate changes do not authorize compilation.
