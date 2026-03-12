// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "OpenObscureTests",
    platforms: [
        .macOS(.v14),
        .iOS(.v17),
    ],
    targets: [
        // C target: FFI header + static library linking
        .target(
            name: "COpenObscure",
            path: "COpenObscure",
            publicHeadersPath: "include",
            linkerSettings: [
                .unsafeFlags(["-L", "lib"]),
                .linkedLibrary("openobscure_core"),
                .linkedLibrary("c++"),
            ]
        ),
        // Swift target: UniFFI-generated bindings
        .target(
            name: "OpenObscure",
            dependencies: ["COpenObscure"],
            path: "OpenObscure"
        ),
        // Executable test runner (works without Xcode — no XCTest dependency)
        .executableTarget(
            name: "RunTests",
            dependencies: ["OpenObscure"],
            path: "OpenObscureTests"
        ),
        // XCTest target for Xcode CI (xcodebuild test)
        .testTarget(
            name: "OpenObscureXCTests",
            dependencies: ["OpenObscure"],
            path: "XCTests"
        ),
    ]
)
