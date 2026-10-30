import SwiftUI

/// About & Licenses sheet presenting project metadata, licensing terms, and open-source credits.
public struct PdfAboutView: View {
    @Environment(\.dismiss) private var dismiss

    public init() {}

    public var body: some View {
        NavigationView {
            List {
                Section {
                    VStack(alignment: .center, spacing: 8) {
                        Text("PdfCraft")
                            .font(.largeTitle)
                            .fontWeight(.bold)

                        Text("Version 0.7.0 (Mobile)")
                            .font(.subheadline)
                            .foregroundColor(.secondary)

                        Text("Pure-Rust PDF Engine across Desktop, Web & Mobile")
                            .font(.footnote)
                            .multilineTextAlignment(.center)
                            .foregroundColor(.secondary)
                            .padding(.top, 2)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 12)
                }

                Section(header: Text("Privacy & Security")) {
                    Label("100% On-Device Processing", systemImage: "lock.shield.fill")
                    Label("Zero Telemetry or Analytics", systemImage: "hand.raised.fill")
                    Label("Atomic & Safe Document Saves", systemImage: "doc.badge.arrow.up.fill")
                }

                Section(header: Text("Licence")) {
                    Text("PdfCraft is free and open-source software, dual-licensed under the MIT License and the Apache License, Version 2.0.")
                        .font(.caption)
                        .foregroundColor(.secondary)

                    NavigationLink("MIT License") {
                        ScrollView {
                            Text(mitLicenseText)
                                .font(.system(.caption, design: .monospaced))
                                .padding()
                        }
                        .navigationTitle("MIT License")
                    }

                    NavigationLink("Apache 2.0 License") {
                        ScrollView {
                            Text(apacheLicenseText)
                                .font(.system(.caption, design: .monospaced))
                                .padding()
                        }
                        .navigationTitle("Apache 2.0")
                    }
                }

                Section(header: Text("Third-Party Acknowledgments")) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Lucide Icons")
                            .font(.subheadline)
                            .fontWeight(.medium)
                        Text("Licensed under the ISC License. Copyright (c) Lucide Contributors.")
                            .font(.caption2)
                            .foregroundColor(.secondary)
                    }

                    VStack(alignment: .leading, spacing: 4) {
                        Text("Open Typography")
                            .font(.subheadline)
                            .fontWeight(.medium)
                        Text("Inter, JetBrains Mono, Dancing Script licensed under the SIL Open Font License 1.1.")
                            .font(.caption2)
                            .foregroundColor(.secondary)
                    }
                }
            }
            .navigationTitle("About PdfCraft")
            #if os(iOS)
            .listStyle(.insetGrouped)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") {
                        dismiss()
                    }
                }
            }
        }
    }

    private var mitLicenseText: String {
        """
        Permission is hereby granted, free of charge, to any person obtaining a copy
        of this software and associated documentation files (the "Software"), to deal
        in the Software without restriction, including without limitation the rights
        to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
        copies of the Software, and to permit persons to whom the Software is
        furnished to do so, subject to the following conditions:

        The above copyright notice and this permission notice shall be included in all
        copies or substantial portions of the Software.

        THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
        IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
        FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
        """
    }

    private var apacheLicenseText: String {
        """
        Licensed under the Apache License, Version 2.0 (the "License");
        you may not use this file except in compliance with the License.
        You may obtain a copy of the License at

            http://www.apache.org/licenses/LICENSE-2.0

        Unless required by applicable law or agreed to in writing, software
        distributed under the License is distributed on an "AS IS" BASIS,
        WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
        See the License for the specific language governing permissions and
        limitations under the License.
        """
    }
}
