// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "preceipts",
    platforms: [.macOS(.v14)],
    dependencies: [
        // Tree-sitter C runtime (used directly — UTF-8 byte offsets, no
        // Swift wrapper layer). Grammars are pinned to the same versions
        // as the Rust registry (core/Cargo.lock) so the ported highlight
        // queries keep matching the grammars' node types.
        .package(url: "https://github.com/tree-sitter/tree-sitter", from: "0.25.0"),
        .package(url: "https://github.com/tree-sitter/tree-sitter-typescript", exact: "0.23.2"),
        .package(url: "https://github.com/tree-sitter/tree-sitter-javascript", exact: "0.23.1"),
        .package(url: "https://github.com/tree-sitter/tree-sitter-rust", exact: "0.23.3"),
        .package(url: "https://github.com/tree-sitter/tree-sitter-json", exact: "0.24.8"),
        .package(url: "https://github.com/tree-sitter/tree-sitter-python", exact: "0.23.6"),
        .package(url: "https://github.com/tree-sitter/tree-sitter-go", exact: "0.23.4"),
        .package(url: "https://github.com/tree-sitter/tree-sitter-css", exact: "0.23.2"),
        .package(url: "https://github.com/tree-sitter/tree-sitter-html", exact: "0.23.2"),
        .package(url: "https://github.com/tree-sitter/tree-sitter-bash", exact: "0.23.3"),
        .package(
            url: "https://github.com/tree-sitter-grammars/tree-sitter-markdown", exact: "0.3.2"),
        .package(url: "https://github.com/tree-sitter-grammars/tree-sitter-toml", exact: "0.7.0"),
        .package(url: "https://github.com/tree-sitter/tree-sitter-c-sharp", exact: "0.23.1"),
        .package(url: "https://github.com/tree-sitter/tree-sitter-ruby", exact: "0.23.1"),
        .package(url: "https://github.com/elixir-lang/tree-sitter-elixir", exact: "0.3.4"),
        // GFM parser (Apple's cmark-gfm binding) for PR comment bodies —
        // structured tables/lists/emphasis; rendering stays native (our
        // NSAttributedString visitor), HTML noise stays sanitized out.
        .package(url: "https://github.com/swiftlang/swift-markdown", exact: "0.8.0"),
    ],
    targets: [
        // Homebrew libgit2 via pkg-config; vendored/static build comes with
        // app-bundle packaging.
        .systemLibrary(
            name: "Clibgit2",
            pkgConfig: "libgit2",
            providers: [.brew(["libgit2"])]
        ),
        .target(
            name: "PreceiptsKit",
            dependencies: [
                "Clibgit2",
                .product(name: "TreeSitter", package: "tree-sitter"),
                .product(name: "TreeSitterTypeScript", package: "tree-sitter-typescript"),
                .product(name: "TreeSitterJavaScript", package: "tree-sitter-javascript"),
                .product(name: "TreeSitterRust", package: "tree-sitter-rust"),
                .product(name: "TreeSitterJSON", package: "tree-sitter-json"),
                .product(name: "TreeSitterPython", package: "tree-sitter-python"),
                .product(name: "TreeSitterGo", package: "tree-sitter-go"),
                .product(name: "TreeSitterCSS", package: "tree-sitter-css"),
                .product(name: "TreeSitterHTML", package: "tree-sitter-html"),
                .product(name: "TreeSitterBash", package: "tree-sitter-bash"),
                .product(name: "TreeSitterMarkdown", package: "tree-sitter-markdown"),
                .product(name: "TreeSitterTOML", package: "tree-sitter-toml"),
                .product(name: "TreeSitterCSharp", package: "tree-sitter-c-sharp"),
                .product(name: "TreeSitterRuby", package: "tree-sitter-ruby"),
                .product(name: "TreeSitterElixir", package: "tree-sitter-elixir"),
            ],
            // The kit is the hot path (parse + diff every changed file on
            // each reload); -Onone made highlighting ~9x slower, so keep it
            // optimized even in debug builds. Drop the flag temporarily if
            // stepping through kit code.
            swiftSettings: [.unsafeFlags(["-O"], .when(configuration: .debug))]
        ),
        .executableTarget(
            name: "PreceiptsApp",
            dependencies: [
                "PreceiptsKit",
                .product(name: "Markdown", package: "swift-markdown"),
            ]),
        .testTarget(name: "PreceiptsKitTests", dependencies: ["PreceiptsKit"]),
    ]
)
