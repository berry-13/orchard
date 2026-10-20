// swift-tools-version:5.3

import PackageDescription

let package = Package(
  name: "tauri-plugin-orchard-pairing",
  platforms: [
    .iOS(.v14)
  ],
  products: [
    .library(
      name: "tauri-plugin-orchard-pairing",
      type: .static,
      targets: ["tauri-plugin-orchard-pairing"])
  ],
  dependencies: [
    .package(name: "Tauri", path: "../.tauri/tauri-api")
  ],
  targets: [
    .target(
      name: "tauri-plugin-orchard-pairing",
      dependencies: [
        .byName(name: "Tauri")
      ],
      path: "Sources")
  ]
)
