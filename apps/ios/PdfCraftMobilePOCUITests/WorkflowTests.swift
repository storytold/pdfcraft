import XCTest
import UIKit

@MainActor
final class WorkflowTests: XCTestCase {
    func testImportCancellationAndMalformedPDFRecovery() {
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launch()
        chooseOrganize(in: app)
        let picker = app.otherElements["Browse View (Picker)"]
        XCTAssertTrue(picker.waitForExistence(timeout: 30), app.debugDescription)
        let cancel = app.descendants(matching: .any).matching(NSPredicate(format: "label == 'Cancel'")).firstMatch
        XCTAssertTrue(cancel.waitForExistence(timeout: 10), app.debugDescription)
        cancel.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        XCTAssertTrue(picker.waitForNonExistence(timeout: 15))
        XCTAssertFalse(app.alerts.firstMatch.exists, "Cancelling Files is not an import failure")
        XCTAssertTrue(app.staticTexts["Welcome to PdfCraft"].exists)

        app.buttons["Open PDF"].tap()
        browseToAppFolder(app)
        openDocument("PdfCraft iOS Invalid Fixture", in: app)
        let alert = app.alerts["PdfCraft"]
        XCTAssertTrue(alert.waitForExistence(timeout: 20), app.debugDescription)
        XCTAssertGreaterThan(alert.staticTexts.count, 1, "Import errors must explain the failure")
        alert.buttons["OK"].tap()
        XCTAssertTrue(app.staticTexts["Welcome to PdfCraft"].waitForExistence(timeout: 10))
        app.buttons["Open PDF"].tap()
        browseToAppFolder(app)
        openDocument("PdfCraft iOS Text Fixture", in: app)
        XCTAssertTrue(app.buttons["Read text on page 1"].waitForExistence(timeout: 30), app.debugDescription)
        XCTAssertFalse(app.descendants(matching: .any)["organize-pages-panel"].firstMatch.exists,
                       "A cancelled tool-first import must not leak its pending tool")
        app.buttons["Read text on page 1"].tap()
        XCTAssertTrue(app.staticTexts["page-text-1"].waitForExistence(timeout: 20), app.debugDescription)
        XCTAssertEqual(app.staticTexts["page-text-1"].label, "PdfCraft text reading\ncaf\u{e9}")
        app.buttons["Done"].tap()
        app.buttons["Close"].tap()
        XCTAssertTrue(app.staticTexts["Welcome to PdfCraft"].waitForExistence(timeout: 10))
    }

    func testMultiPageSelectionZoomRotationAndLandscape() {
        continueAfterFailure = false
        XCUIDevice.shared.orientation = .portrait
        defer { XCUIDevice.shared.orientation = .portrait }
        let app = XCUIApplication()
        app.launch()
        chooseOrganize(in: app)
        browseToAppFolder(app)
        openDocument("PdfCraft iOS Multipage Fixture", in: app)
        let first = app.descendants(matching: .any)["rendered-page-1"].firstMatch
        XCTAssertTrue(first.waitForExistence(timeout: 30), app.debugDescription)
        let fitWidth = first.frame.width
        let zoom = app.buttons["Zoom page 1"]
        XCTAssertEqual(zoom.value as? String, "100%")
        zoom.tap()
        app.buttons["Zoom in"].tap()
        let zoomed = NSPredicate { _, _ in first.frame.width > fitWidth * 1.3 }
        expectation(for: zoomed, evaluatedWith: nil)
        waitForExpectations(timeout: 10)
        XCTAssertEqual(zoom.value as? String, "150%")
        zoom.tap()
        app.buttons["Fit page"].tap()
        XCTAssertEqual(first.frame.width, fitWidth, accuracy: 2)
        XCTAssertEqual(zoom.value as? String, "100%")
        app.buttons["Page 2"].firstMatch.tap()
        XCTAssertTrue(app.staticTexts["Page 2 of 2"].waitForExistence(timeout: 10), app.debugDescription)
        let second = app.descendants(matching: .any)["rendered-page-2"].firstMatch
        XCTAssertTrue(second.waitForExistence(timeout: 30), app.debugDescription)
        XCTAssertTrue(second.isHittable, "Selecting a page must scroll it into view")
        app.buttons["Read text on page 2"].tap()
        XCTAssertTrue(app.staticTexts["page-text-2"].waitForExistence(timeout: 20), app.debugDescription)
        XCTAssertEqual(app.staticTexts["page-text-2"].label, "PdfCraft second page\nSecond page only")
        app.buttons["Done"].tap()
        app.buttons["Rotate page 2 clockwise"].tap()
        let rotated = NSPredicate { _, _ in second.exists && second.frame.width > second.frame.height }
        expectation(for: rotated, evaluatedWith: nil)
        waitForExpectations(timeout: 30)
        app.buttons["Page 1"].firstMatch.tap()
        XCTAssertTrue(app.staticTexts["Page 1 of 2"].waitForExistence(timeout: 10))
        XCTAssertTrue(first.waitForExistence(timeout: 30))
        XCTAssertLessThan(first.frame.width, first.frame.height, "Rotating page 2 must not rotate page 1")
        XCUIDevice.shared.orientation = .landscapeLeft
        let landscape = NSPredicate { _, _ in app.frame.width > app.frame.height }
        expectation(for: landscape, evaluatedWith: nil)
        waitForExpectations(timeout: 10)
        XCTAssertTrue(app.buttons["Read text on page 1"].isHittable, app.debugDescription)
        XCTAssertGreaterThan(first.frame.intersection(app.frame).height, 100,
                             "Landscape controls must leave a usable document viewport\n" + app.debugDescription)
        attachScreenshot(app, named: "Multipage landscape")
        XCUIDevice.shared.orientation = .portrait
        shareAndCopy(in: app)
        app.buttons["Close"].tap()
        XCTAssertTrue(app.buttons["Discard and Close"].waitForExistence(timeout: 10),
                      "Sharing a staged copy must not clear the unsaved-changes warning")
        app.buttons["Discard and Close"].tap()
        XCTAssertTrue(app.staticTexts["Welcome to PdfCraft"].waitForExistence(timeout: 10))
    }

    func testAccessibilityTextSizeKeepsDocumentControlsReachable() {
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launchArguments += ["-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL"]
        app.launch()
        chooseOrganize(in: app)
        browseToAppFolder(app)
        openDocument("PdfCraft iOS Text Fixture", in: app)
        let page = app.descendants(matching: .any)["rendered-page-1"].firstMatch
        XCTAssertTrue(page.waitForExistence(timeout: 30), app.debugDescription)
        XCTAssertGreaterThan(page.frame.intersection(app.frame).height, 100, app.debugDescription)
        XCTAssertTrue(app.buttons["Read text on page 1"].isHittable, app.debugDescription)
        let save = app.toolbars.buttons["Save a Copy"]
        attachScreenshot(app, named: "Accessibility text document controls")
        save.tap()
        cancelExport(in: app)
        app.buttons["Read text on page 1"].tap()
        XCTAssertTrue(app.staticTexts["page-text-1"].waitForExistence(timeout: 20), app.debugDescription)
        XCTAssertTrue(app.buttons["Done"].isHittable)
        app.buttons["Done"].tap()
        app.buttons["Close"].tap()
        XCTAssertTrue(app.staticTexts["Welcome to PdfCraft"].waitForExistence(timeout: 10))
    }

    func testDesktopWorkspaceSearchPinsAppearanceAndToolFirstOpen() {
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launch()
        XCTAssertTrue(app.staticTexts["Welcome to PdfCraft"].waitForExistence(timeout: 20), app.debugDescription)
        if UIDevice.current.userInterfaceIdiom == .pad {
            XCTAssertTrue(app.descendants(matching: .any)["workspace-sidebar"].firstMatch.exists)
        }
        let appearance = app.buttons["Appearance"]
        let previousAppearance = appearance.value as? String ?? "System"
        appearance.tap()
        app.buttons["Dark"].tap()
        XCTAssertEqual(appearance.value as? String, "Dark")

        app.buttons["Find tools and commands"].tap()
        let search = app.searchFields.firstMatch
        XCTAssertTrue(search.waitForExistence(timeout: 10), app.debugDescription)
        search.tap()
        search.typeText("rotate")
        let browser = app.scrollViews["tools-browser"]
        let organize = browser.buttons["tool-page.organize"]
        XCTAssertTrue(organize.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertFalse(browser.buttons["tool-mobile.share"].exists, "Search must filter nonmatching tools")
        let unpin = browser.buttons["Unpin Organize pages from Home"]
        let wasPinned = unpin.exists
        if !wasPinned { browser.buttons["Pin Organize pages to Home"].tap() }
        XCTAssertTrue(unpin.exists)
        if !app.buttons["Done"].exists {
            let cancelSearch = app.buttons.matching(NSPredicate(format: "label == 'Cancel' OR label == 'close'")).firstMatch
            XCTAssertTrue(cancelSearch.waitForExistence(timeout: 5), app.debugDescription)
            cancelSearch.tap()
        }
        app.buttons["Done"].tap()

        app.terminate()
        app.launch()
        XCTAssertTrue(app.staticTexts["Welcome to PdfCraft"].waitForExistence(timeout: 20))
        XCTAssertEqual(appearance.value as? String, "Dark")
        app.buttons["Find tools and commands"].tap()
        XCTAssertTrue(unpin.waitForExistence(timeout: 10), "Pins must survive relaunch\n" + app.debugDescription)
        if !wasPinned { unpin.tap() }
        organize.tap()
        browseToAppFolder(app)
        openDocument("PdfCraft iOS UI Fixture", in: app)
        XCTAssertTrue(app.descendants(matching: .any)["organize-pages-panel"].firstMatch.waitForExistence(timeout: 20),
                      "Choosing a tool before opening a file must resume that tool\n" + app.debugDescription)
        XCTAssertTrue(app.buttons["Rotate page 1 clockwise"].exists)
        let pageImage = app.descendants(matching: .any)["rendered-page-1"].firstMatch
        XCTAssertTrue(pageImage.waitForExistence(timeout: 30), app.debugDescription)
        XCTAssertTrue(pageImage.isHittable, "Organize controls must leave the PDF visible")
        app.buttons["Close"].tap()
        XCTAssertTrue(app.staticTexts["Welcome to PdfCraft"].waitForExistence(timeout: 10))
        appearance.tap()
        app.buttons[previousAppearance].tap()
        XCTAssertEqual(appearance.value as? String, previousAppearance)
    }

    func testFilesRotateExportReopenAndShare() {
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launch()
        XCTAssertTrue(app.buttons["Open PDF"].waitForExistence(timeout: 20))
        app.buttons["Open PDF"].tap()
        browseToAppFolder(app)
        openDocument("PdfCraft iOS UI Fixture", in: app)
        let image = app.descendants(matching: .any)["rendered-page-1"].firstMatch
        XCTAssertTrue(image.waitForExistence(timeout: 30), app.debugDescription)
        XCTAssertLessThan(image.frame.width, image.frame.height)
        app.buttons["Read text on page 1"].tap()
        XCTAssertTrue(app.staticTexts["page-text-empty"].waitForExistence(timeout: 20), app.debugDescription)
        app.buttons["Done"].tap()
        app.buttons["Rotate page 1 clockwise"].tap()
        let landscape = NSPredicate { _, _ in image.exists && image.frame.width > image.frame.height }
        expectation(for: landscape, evaluatedWith: nil)
        waitForExpectations(timeout: 30)

        app.toolbars.buttons["Save a Copy"].tap()
        cancelExport(in: app)
        app.buttons["Close"].coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        XCTAssertTrue(app.buttons["Discard and Close"].waitForExistence(timeout: 10),
                      "Cancelling export must retain the unsaved-changes warning\n" + app.debugDescription)
        image.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        XCTAssertTrue(app.buttons["Discard and Close"].waitForNonExistence(timeout: 5))

        app.toolbars.buttons["Save a Copy"].tap()
        browseToAppFolder(app)
        let save = app.navigationBars.buttons.matching(NSPredicate(format: "label == 'Export' OR label == 'Save'")).firstMatch
        XCTAssertTrue(save.waitForExistence(timeout: 10), app.debugDescription)
        save.tap()
        XCTAssertTrue(app.otherElements["Browse View (Picker)"].waitForNonExistence(timeout: 20),
                      app.debugDescription)
        XCTAssertTrue(app.buttons["Close"].waitForExistence(timeout: 20), app.debugDescription)
        app.buttons["Close"].coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        XCTAssertTrue(app.buttons["Open PDF"].waitForExistence(timeout: 10), app.debugDescription)
        app.buttons["Open PDF"].tap()
        browseToAppFolder(app)
        openDocument("copy-2", in: app)
        XCTAssertTrue(image.waitForExistence(timeout: 30), app.debugDescription)
        XCTAssertGreaterThan(image.frame.width, image.frame.height)

        shareAndCopy(in: app)
    }

    func testReadEmbeddedTextBeforeAndAfterRotation() {
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launch()
        XCTAssertTrue(app.buttons["Open PDF"].waitForExistence(timeout: 20))
        app.buttons["Open PDF"].tap()
        browseToAppFolder(app)
        openDocument("PdfCraft iOS Text Fixture", in: app)
        let read = app.buttons["Read text on page 1"]
        XCTAssertTrue(read.waitForExistence(timeout: 30), app.debugDescription)
        read.tap()
        let text = app.staticTexts["page-text-1"]
        XCTAssertTrue(text.waitForExistence(timeout: 20), app.debugDescription)
        XCTAssertEqual(text.label, "PdfCraft text reading\ncaf\u{e9}")
        XCTAssertFalse(app.staticTexts["page-text-warning"].exists)
        text.press(forDuration: 1)
        let copy = app.menuItems["Copy"].waitForExistence(timeout: 3) ? app.menuItems["Copy"] : app.buttons["Copy"]
        XCTAssertTrue(copy.waitForExistence(timeout: 5), app.debugDescription)
        copy.tap()
        XCTAssertTrue(copy.waitForNonExistence(timeout: 5), app.debugDescription)
        app.buttons["Done"].tap()
        XCTAssertTrue(text.waitForNonExistence(timeout: 10))
        app.buttons["Rotate page 1 clockwise"].tap()
        let image = app.descendants(matching: .any)["rendered-page-1"].firstMatch
        let landscape = NSPredicate { _, _ in image.exists && image.frame.width > image.frame.height }
        expectation(for: landscape, evaluatedWith: nil)
        waitForExpectations(timeout: 30)
        read.tap()
        XCTAssertTrue(text.waitForExistence(timeout: 20), app.debugDescription)
        XCTAssertEqual(text.label, "PdfCraft text reading\ncaf\u{e9}")
        app.buttons["Done"].tap()
        XCTAssertTrue(text.waitForNonExistence(timeout: 10))
    }

    private func chooseOrganize(in app: XCUIApplication) {
        XCTAssertTrue(app.buttons["Find tools and commands"].waitForExistence(timeout: 20))
        app.buttons["Find tools and commands"].tap()
        let organize = app.scrollViews["tools-browser"].buttons["tool-page.organize"]
        XCTAssertTrue(organize.waitForExistence(timeout: 10), app.debugDescription)
        organize.tap()
    }

    private func cancelExport(in app: XCUIApplication) {
        let exportBar = app.navigationBars.containing(.button, identifier: "Save").firstMatch
        XCTAssertTrue(exportBar.waitForExistence(timeout: 15), app.debugDescription)
        if UIDevice.current.userInterfaceIdiom == .pad {
            let cancel = app.navigationBars.buttons["Cancel"]
            XCTAssertTrue(cancel.waitForExistence(timeout: 10), app.debugDescription)
            cancel.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        } else {
            exportBar.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0))
                .press(forDuration: 0.1, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.85)))
        }
        XCTAssertTrue(exportBar.waitForNonExistence(timeout: 20), app.debugDescription)
    }

    private func shareAndCopy(in app: XCUIApplication) {
        app.toolbars.buttons["Share"].tap()
        let copy = app.cells.matching(NSPredicate(format: "label == 'Copy'")).firstMatch
        XCTAssertTrue(copy.waitForExistence(timeout: 15), app.debugDescription)
        let copyIcon = copy.images.firstMatch
        XCTAssertTrue(copyIcon.waitForExistence(timeout: 10), app.debugDescription)
        copyIcon.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        XCTAssertTrue(copy.waitForNonExistence(timeout: 15))
        XCTAssertTrue(app.buttons["Close"].waitForExistence(timeout: 15))
    }

    private func attachScreenshot(_ app: XCUIApplication, named name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    private func browseToAppFolder(_ app: XCUIApplication) {
        let picker = app.otherElements["Browse View (Picker)"]
        XCTAssertTrue(picker.waitForExistence(timeout: 60), app.debugDescription)
        let testFolder = "PdfCraft UI Workflow Test"
        let currentFolder = picker.navigationBars.descendants(matching: .any)
            .matching(NSPredicate(format: "label == %@ OR label == %@",
                                 testFolder, testFolder + ", Actions Menu")).firstMatch
        if currentFolder.waitForExistence(timeout: 3) { return }
        let browse = app.buttons["Browse"].firstMatch
        if browse.waitForExistence(timeout: 5) { browse.tap() }
        let location = UIDevice.current.userInterfaceIdiom == .pad ? "On My iPad" : "On My iPhone"
        let local = picker.staticTexts[location]
        if local.waitForExistence(timeout: 5) { local.tap() }
        let folder = picker.staticTexts["PdfCraft"].firstMatch
        if folder.waitForExistence(timeout: 5) { folder.tap() }
        let workflow = picker.cells.matching(NSPredicate(format: "label BEGINSWITH %@", testFolder + ","))
            .images.firstMatch
        XCTAssertTrue(workflow.waitForExistence(timeout: 10), app.debugDescription)
        workflow.tap()
        XCTAssertTrue(currentFolder.waitForExistence(timeout: 10), app.debugDescription)
    }

    private func openDocument(_ name: String, in app: XCUIApplication) {
        let item = app.cells[name + ", pdf"].images.firstMatch
        XCTAssertTrue(item.waitForExistence(timeout: 15), app.debugDescription)
        let picker = app.otherElements["Browse View (Picker)"]
        var visible = false
        for _ in 0..<6 {
            let top = picker.navigationBars.firstMatch.exists ? picker.navigationBars.firstMatch.frame.maxY : picker.frame.minY
            let bottom = app.tabBars.firstMatch.exists ? app.tabBars.firstMatch.frame.minY : picker.frame.maxY
            visible = item.frame.minY >= top && item.frame.maxY <= bottom
            if visible { break }
            let endY: CGFloat = item.frame.minY < top ? 0.7 : 0.3
            picker.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
                .press(forDuration: 0.05, thenDragTo: picker.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: endY)),
                       withVelocity: .slow, thenHoldForDuration: 0.5)
        }
        XCTAssertTrue(visible, "The PDF thumbnail must be visible before tapping\n" + app.debugDescription)
        // Files can report an invalid accessibility hit point for a visible thumbnail.
        let thumbnail = item.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        if UIDevice.current.userInterfaceIdiom == .phone {
            // Compact Simulator Files sheets can ignore a single synthetic tap.
            thumbnail.doubleTap()
        } else {
            thumbnail.tap()
        }
        XCTAssertTrue(app.otherElements["Browse View (Picker)"].waitForNonExistence(timeout: 20),
                      app.debugDescription)
    }
}
