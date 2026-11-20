# Native workflow canvas evidence

These are real `XCTAttachment(screenshot:)` exports from
`WorkflowTests.testFilesRotateExportReopenAndShare`, not generated mockups.
Runtime/harness source: `7d8609279768337bdbf2e5ef7014e6e7d96ca6d7`.
The original attachment is named `Reopened rotated export canvas`.

| Image | Destination | What was captured |
| --- | --- | --- |
| [iPhone](iphone-reopened-export.png) | iPhone 18 Pro, iOS 27 | Rendered page after native Files import, rotation, cancelled export, actual export and reopen |
| [iPad](ipad-reopened-export.png) | iPad Pro 11-inch (M5), iOS 27 | The same real workflow on the tablet |

Only the rendered image element is captured. Its geometric, font-free synthetic
PDF comes from the repository's hosted-test fixture; the blue selection border is
the app's own drawing. No personal document, provider/account UI, proprietary
font, native symbol or OS chrome is published. PNG profile metadata is removed
before publication; the asset manifest records the resulting file hashes.

The full native UI was exercised by seven UI cases per family, including Home,
tools/search/pins/appearance, embedded text selection/copy, multipage navigation,
zoom/landscape, accessibility text, cancellation, nine cancelled exports followed
by a tenth saved/reopened copy, dirty-close confirmation and sharing. A canvas
image alone is not proof of those other assertions; see the retained result
summaries and [review](../../apple-platform-review.md).

Full native-chrome captures remain private under the current stricter asset
policy. The SDK-resource/screenshot policy decision is requested in
[storytold/craft-contrib#63](https://github.com/storytold/craft-contrib/pull/63),
not assumed accepted. These images do not establish physical-device,
minimum-OS, VoiceOver speech, hosted-runner or Store acceptance.

To reproduce, run the [native suites](../../../apps/ios/README.md#vs-code--command-line-workflow),
then export only the permitted attachment:

```sh
xcrun xcresulttool export attachments --filter '*canvas*' \
  --path <retained-run>/ui.xcresult --output-path <private-export-directory>
```

Inspect the manifest's device/test identifiers and the actual pixels before
publishing. Never substitute a full native-chrome or Files/provider capture.
