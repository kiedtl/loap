---
page: About
---

## About

LOAP provides prebuilt tarballs for [KISS
Linux](https://kisscommunity.bvnf.space). They're built by volunteers in their
own time on their machines, not on any dedicated build host; packages are stored
on a free 10GB Backblaze B2 bucket. Because of these constraints, only specific
packages are included (see the criteria below).

New volunteers would be appreciated; please ping kiedtl in the `#kisslinux` IRC
channel.

**Attention**: This project is **experimental**; no guarantees of function or
continued operation are provided!

### Criteria

This service is meant for packages which

- Take an inordinately long time to build, or
- Have a number of **build-only** dependencies which take a long time to build,
  or pollute the builder's machine.

Packages which build quickly (such as `repo/extra/opendoas`, which can be
installed in less than *1 second*) will not be included here.

Packages which will not be commonly used will most likely not be included here,
unless someone volunteers for it. This will include many language toolchains,
niche browsers, etc.
