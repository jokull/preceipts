// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "preceipts",
    platforms: [.macOS(.v14)],
    targets: [
        // Homebrew libgit2 via pkg-config; vendored/static build comes with
        // app-bundle packaging.
        .systemLibrary(
            name: "Clibgit2",
            pkgConfig: "libgit2",
            providers: [.brew(["libgit2"])]
        ),
        .target(name: "PreceiptsKit", dependencies: ["Clibgit2"]),
        .executableTarget(name: "PreceiptsApp", dependencies: ["PreceiptsKit"]),
        .testTarget(name: "PreceiptsKitTests", dependencies: ["PreceiptsKit"]),
    ]
)
