# kiss-pig

A command-line tool to streamline downloading tarballs from LOAP and putting it
in `kiss`'s cache.

## Usage

    $ kiss-pig <package> [version]
    $ kiss-pig opendoas
    $ kiss-pig opendoas 6.8.2-1

NOTE: `version` must contain the package revision number, i.e. the "-1" at
the end.

If `version` is not provided, `KISS_PATH` will be searched for the package.

## Downloads

Binaries are available from the GitHub releases.

Ironically, it isn't available in LOAP, since `kiss-pig` isn't in any repository
and I don't want to create one just for this.

## Building

Requirements: Zig 0.14.0, libcurl.

Compilation:

    $ zig build --release ReleaseSafe

The binary will be in `zig-out/bin/kiss-pig`.
