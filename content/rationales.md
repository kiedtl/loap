---
page: "rationales"
---

## Rationales

Justifications for some packages being included here.

### Firefox

1. Literally the reason this project was created in the first place.

### Rust, LLVM, Clang

1. Take a fairly long time build.
2. Rust provides `cargo`, which can be used to compile and install many other
   packages not provided by the repositories.

### Mesa

1. Doesn't take long to compile, but
2. Has 11(!) build dependencies.

### Zlib

1. Compiles very quickly, but
2. Is the most common non-build dependency (for packages in `kiss-community/*`).
