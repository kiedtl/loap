---
page: Faq
---

## FAQ

- **What architectures are supported?**

Newer `x86_64` machines, see `CFLAGS` below.

- **What `CFLAGS` are used?**

The goal is to build with optimizations without compromising on the number of
supported machines. To that end, `-O2 -march=x86-64-v2` is used. Only very old
x86_64 machines will not work with these tarballs.

For Rust packages, the `RUSTFLAGS` equivalent is used.

- **What about Aarch64/RISC-V/older Intel machines?**

Arguably such computers are in most need of prebuilt packages, since it'd be
more difficult than usual to compile packages on the machines itself. But this
won't be done unless volunteers and extra storage space is discovered somehow.

- **How do I report issues?**

Ping the maintainer in IRC or send an email.

- **How long will old versions be available?**

As of now that hasn't been decided. Most likely, as the 10GB limit becomes more
imminent, older versions of larger packages (`llvm`, `clang`, `rust`) will begin
to be deleted.

- **What relation does LOAP have to the KISS project?**

LOAP was made by kiedtl, a former KISS community maintainer, who recently
returned to the project (hello \_o/). It has not been explicitly endorsed by
KISS community.

LOAP volunteers may or may not be part of the "official" KISS community.

Needless to say, it has not been endorsed by Dylan.
