const std = @import("std");

pub fn build(b: *std.Build) void {
    const known_folders = b.dependency("known_folders", .{}).module("known-folders");
    const clap = b.dependency("clap", .{}).module("clap");
    const curl = b.dependency("curl", .{ .link_vendor = false }).module("curl");

    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const exe_mod = b.createModule(.{
        .root_source_file = b.path("src/main.zig"),
        .target = target,
        .optimize = optimize,
    });

    const exe = b.addExecutable(.{ .name = "kiss-pig", .root_module = exe_mod });
    exe.linkLibC();
    exe.linkSystemLibrary("curl");
    exe.root_module.addImport("known-folders", known_folders);
    exe.root_module.addImport("clap", clap);
    exe.root_module.addImport("curl", curl);
    b.installArtifact(exe);

    const run_cmd = b.addRunArtifact(exe);
    run_cmd.step.dependOn(b.getInstallStep());
    if (b.args) |args| run_cmd.addArgs(args);
    const run_step = b.step("run", "Run the app");
    run_step.dependOn(&run_cmd.step);
}
