// swift-tools-version: 5.9

import PackageDescription

let package = Package(
    name: "McpLocalRelayStatusBar",
    platforms: [.macOS(.v13)],
    products: [
        .executable(name: "McpLocalRelayStatusBar", targets: ["McpLocalRelayStatusBar"])
    ],
    targets: [
        .executableTarget(name: "McpLocalRelayStatusBar")
    ]
)
