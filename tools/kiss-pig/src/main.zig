const std = @import("std");
const fmt = std.fmt;
const fs = std.fs;
const json = std.json;
const mem = std.mem;
const process = std.process;

const c_alloc = std.heap.c_allocator;
const kf = @import("known-folders");
const clap = @import("clap");
const curl = @import("curl");
const c = curl.libcurl;

pub const std_options = std.Options{
    .logFn = myLogFn,
};

pub fn main() void {
    var arena = std.heap.ArenaAllocator.init(c_alloc);
    defer arena.deinit();
    const alloc = arena.allocator();

    const HELP =
        \\Usage: kiss pig [options] <PKG> [VERSION]
        \\
        \\Version must be in the <package>-<revision> format
        \\e.g. 1.2.3-1 (instead of "1.2.3 1"). If not provided,
        \\KISS_PATH will be searched for the package directory.
        \\
        \\Options:
        \\     -h, --help          Show this message.
        \\
        \\
    ;

    const params = comptime clap.parseParamsComptime(
        \\-h, --help
        \\<str>
        \\<str>
    );

    var diag = clap.Diagnostic{};
    var res = clap.parse(clap.Help, &params, clap.parsers.default, .{
        .diagnostic = &diag,
        .allocator = alloc,
    }) catch |err| {
        diag.report(std.io.getStdErr().writer(), err) catch {};
        return;
    };
    defer res.deinit();

    if (res.args.help != 0) {
        std.io.getStdErr().writer().print("{s}", .{HELP}) catch {};
        return;
    }

    const package = res.positionals[0] orelse {
        std.io.getStdErr().writer().print("{s}", .{HELP}) catch {};
        return;
    };
    const version = if (res.positionals[1]) |version| b: {
        break :b version;
    } else b: {
        const kiss_path = KissPath.initAndPopulate(alloc) catch |e|
            die("Couldn't read and parse KISS_PATH: {}", .{e});

        var dir = kiss_path.findPackage(package) catch |e|
            die("Couldn't find package '{s}': {}", .{ package, e }) orelse
            die("Couldn't find package '{s}' in KISS_PATH.", .{package});
        defer dir.close();

        const version_str = dir.readFileAlloc(alloc, "version", 128) catch |e|
            die("Couldn't read `version` file: {}", .{e});

        // LOAP expects versions to be in the format used in file names, i.e. 1.2.3-1 not "1.2.3 1"
        for (version_str) |*ch|
            if (ch.* == ' ') {
                ch.* = '-';
            };

        break :b mem.trimRight(u8, version_str, "\n ");
    };

    const mycurl = c.curl_easy_init() orelse @panic("libcurl isn't cooperating");
    defer c.curl_easy_cleanup(mycurl);

    std.log.info("Looking for \x1b[1m{s}\x1b[m@\x1b[1m{s}\x1b[m", .{ package, version });

    const url = fmt.allocPrint(alloc, "https://loap.k1sslinux.org/api/b/ls?p={s}\x00", .{package}) catch
        @panic("Close a few Firefox tabs please");

    _ = c.curl_easy_setopt(mycurl, c.CURLOPT_URL, url.ptr);
    _ = c.curl_easy_setopt(mycurl, c.CURLOPT_FOLLOWLOCATION, @as(u64, 1));

    var buffer = std.ArrayList(u8).init(alloc);
    _ = c.curl_easy_setopt(mycurl, c.CURLOPT_WRITEFUNCTION, struct {
        pub fn f(data: [*c]u8, size: usize, nmemb: usize, buf: *std.ArrayList(u8)) callconv(.C) usize {
            const oldlen = buf.items.len;
            buf.ensureTotalCapacity(oldlen + size * nmemb) catch @panic("OOM");
            buf.items.len += size * nmemb;
            @memcpy(buf.items[oldlen..], data[0 .. size * nmemb]);
            return nmemb * size;
        }
    }.f);
    _ = c.curl_easy_setopt(mycurl, c.CURLOPT_WRITEDATA, &buffer);

    const curl_res = c.curl_easy_perform(mycurl);
    if (curl_res != c.CURLE_OK)
        die("Couldn't complete request: {s}", .{c.curl_easy_strerror(curl_res)});

    var http_code: u64 = undefined;
    _ = c.curl_easy_getinfo(mycurl, c.CURLINFO_RESPONSE_CODE, &http_code);

    if (http_code != 200)
        die("LOAP responded with error: {}; response:\n{s}", .{ http_code, buffer.items });

    const builds_deser = json.parseFromSlice([]Build, alloc, buffer.items, .{
        .ignore_unknown_fields = true,
    }) catch |e|
        die("Couldn't deserialize LOAP's response: {}", .{e});
    const builds = builds_deser.value;

    // TODO: if multiple builds of same version, choose most recent.
    const build = for (builds) |build| {
        if (mem.eql(u8, build.version, version))
            break build;
    } else die("No builds found.", .{});

    std.log.info("Downloading \x1b[1m{s}\x1b[m@\x1b[1m{s}\x1b[m (size {}) built by \x1b[33m{s}\x1b[m.", .{
        package, version, sizefmt(build.size), build.builder_name,
    });

    var cache_dir = kf.open(alloc, .cache, .{}) catch |e|
        die("Couldn't open cache dir: {}", .{e}) orelse
        die("Cache dir doesn't exist, refusing to continue.", .{});
    defer cache_dir.close();

    var kiss_cache_dir = cache_dir.openDir("kiss/bin", .{}) catch |e|
        die("Couldn't open KISS cache dir: {}", .{e});
    defer kiss_cache_dir.close();

    const tarball_fname = fmt.allocPrint(alloc, "{s}@{s}.tar.xz", .{ package, version }) catch @panic("OOM");
    const tarball = kiss_cache_dir.createFile(tarball_fname, .{}) catch |e| {
        std.log.info("Couldn't create {s} for writing: {}.", .{ tarball_fname, e });
        return;
    };
    defer tarball.close();

    // CURLOPT_WRITEDATA wants a FILE*, not a file descriptor unfortunately.
    const tarball_fp = c.fdopen(tarball.handle, "w") orelse @panic("fdopen");

    std.log.info("Waiting for curl...", .{});

    const dl_url = fmt.allocPrint(alloc, "https://loap.k1sslinux.org/api/b/dl?id={}\x00", .{build.id}) catch
        @panic("Close a few Firefox tabs please");
    _ = c.curl_easy_setopt(mycurl, c.CURLOPT_URL, dl_url.ptr);
    _ = c.curl_easy_setopt(mycurl, c.CURLOPT_WRITEDATA, tarball_fp);
    _ = c.curl_easy_setopt(mycurl, c.CURLOPT_WRITEFUNCTION, @as(?*anyopaque, @ptrFromInt(0)));
    _ = c.curl_easy_setopt(mycurl, c.CURLOPT_XFERINFOFUNCTION, struct {
        pub fn f(ctx: *anyopaque, dltotal: i64, dlnow: i64, ultotal: i64, ulnow: i64) callconv(.C) c_int {
            const stderr = std.io.getStdErr();
            stderr.writer().print("\r\x1b[ADownloading: {} / {}, {}%\n", .{
                sizefmt(dlnow), sizefmt(dltotal), @divTrunc(dlnow * 100, @max(1, dltotal)),
            }) catch unreachable;

            // Zig is asshole. Why isn't prefixing the names with _ enough?
            _ = ctx;
            _ = ultotal;
            _ = ulnow;

            return 0;
        }
    }.f);
    // XFERINFOFUNCTION has no effect otherwise
    _ = c.curl_easy_setopt(mycurl, c.CURLOPT_NOPROGRESS, @as(usize, 0));

    const curl_res2 = c.curl_easy_perform(mycurl);
    if (curl_res2 != c.CURLE_OK)
        die("Couldn't complete request: {s}", .{c.curl_easy_strerror(curl_res2)});
}

pub fn die(comptime format: []const u8, args: anytype) noreturn {
    std.log.err(format, args);
    process.exit(1);
}

pub fn myLogFn(
    comptime level: std.log.Level,
    comptime _: @Type(.enum_literal),
    comptime format: []const u8,
    args: anytype,
) void {
    const prefix = "\x1b[34m[kiss-pig]\x1b[m";

    const level_str = switch (level) {
        .debug, .info => "",
        .warn => "\x1b[33m(!!)\x1b[m ",
        .err => "\x1b[1;31merr:\x1b[m ",
    };

    std.debug.lockStdErr();
    defer std.debug.unlockStdErr();
    const stderr = std.io.getStdErr().writer();
    nosuspend stderr.print(prefix ++ " " ++ level_str ++ format ++ "\n", args) catch return;
}

const Build = struct {
    id: usize,
    version: []const u8,
    size: usize,
    completed_at: []const u8,
    downloads: usize,
    builder_name: []const u8,
};

pub fn SizeFmt(comptime T: anytype) type {
    return struct {
        inner: T,

        pub fn format(self: *const @This(), comptime f: []const u8, _: fmt.FormatOptions, w: anytype) !void {
            if (comptime !mem.eql(u8, f, "")) @compileError("Unknown format string: '" ++ f ++ "'");

            const v = @as(f64, @floatFromInt(self.inner));
            try switch (self.inner) {
                0...999 => fmt.format(w, "{d:.0} bytes", .{v}),
                1000...999_999 => fmt.format(w, "{d:.1} KB", .{v / 1000.0}),
                1_000_000...1_000_000_000 => fmt.format(w, "{d:.2} MB", .{v / 1_000_000.0}),
                else => fmt.format(w, "{d:.3} GB", .{v / 1_000_000_000.0}),
            };
        }
    };
}

pub fn sizefmt(value: anytype) SizeFmt(@TypeOf(value)) {
    return .{ .inner = value };
}

const KissPath = struct {
    dirs: std.ArrayList(fs.Dir),

    pub fn initAndPopulate(alloc: mem.Allocator) !KissPath {
        var dirs = std.ArrayList(fs.Dir).init(alloc);

        const env = try process.getEnvVarOwned(alloc, "KISS_PATH");
        defer alloc.free(env);

        var segments = mem.splitScalar(u8, env, ':');
        while (segments.next()) |segment|
            try dirs.append(try fs.cwd().openDir(segment, .{ .iterate = true }));

        return KissPath{ .dirs = dirs };
    }

    // Attempts to open the *first* entry found. If it fails, it returns the
    // relevant error immediately and doesn't continue searching.
    pub fn findPackage(self: *const KissPath, package: []const u8) !?fs.Dir {
        for (self.dirs.items) |dir| {
            var iter = dir.iterate();
            while (try iter.next()) |entry|
                if (mem.eql(u8, entry.name, package) and entry.kind == .directory)
                    return try dir.openDir(package, .{});
        }
        return null;
    }

    pub fn deinit(self: *const KissPath) void {
        self.dirs.deinit();
    }
};
