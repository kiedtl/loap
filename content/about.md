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
- Have a number of **build-only** dependencies which take a long time to build
  or pollute the builder's machine (*mesa*), or
- Are depended on by a large number of packages (*zlib*).

Packages which build quickly (such as `repo/extra/opendoas`, which can be
installed in less than *1 second*) will not be included here.

Packages which will not be commonly used will most likely not be included here,
unless someone volunteers for it. This will include many language toolchains,
niche browsers, etc.

See also: [rationales](/rationales)

### Guarantees

LOAP is provided "as-is", with no hard guarantee the hosted packages will work
for you.

KISS is a diverse platform. Nearly every system component can be swapped out
with relative ease, and the configuration of existing packages can be changed by
modifying a single build script. It's probably not possible for LOAP's tarballs
to work everywhere.

Even on relatively unmodified systems, the fact that LOAP is run by a small
group of volunteers means that we don't have the means to perform extensive
testing.

**This doesn't mean bug reports aren't welcome.** LOAP is hosted with the intent of
being maximally useful with minimal effort, not for catfishing newbies with fake
promises of an ultra-mnml Arch Linux experience. If you find a package that
isn't working, and you're pretty sure it *should* work on your system, please
ping someone in the IRC channel or send an email to the maintainer.
