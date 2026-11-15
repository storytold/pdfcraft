import XCTest
import UIKit

@MainActor
final class WorkflowTests: XCTestCase {
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
        app.buttons["Rotate page 1 clockwise"].tap()
        let landscape = NSPredicate { _, _ in image.exists && image.frame.width > image.frame.height }
        expectation(for: landscape, evaluatedWith: nil)
        waitForExpectations(timeout: 30)

        app.buttons["Save a Copy"].tap()
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
        XCTAssertTrue(app.otherElements["Browse View (Picker)"].waitForNonExistence(timeout: 20),
                      app.debugDescription)
        app.buttons["Close"].coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        XCTAssertTrue(app.buttons["Discard and Close"].waitForExistence(timeout: 10),
                      "Cancelling export must retain the unsaved-changes warning\n" + app.debugDescription)
        image.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        XCTAssertTrue(app.buttons["Discard and Close"].waitForNonExistence(timeout: 5))

        app.buttons["Save a Copy"].tap()
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

        app.buttons["Share"].tap()
        let copy = app.cells.matching(NSPredicate(format: "label == 'Copy'")).firstMatch
        XCTAssertTrue(copy.waitForExistence(timeout: 15), app.debugDescription)
        let copyIcon = copy.images.firstMatch
        XCTAssertTrue(copyIcon.waitForExistence(timeout: 10), app.debugDescription)
        copyIcon.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        XCTAssertTrue(copy.waitForNonExistence(timeout: 15))
        XCTAssertTrue(app.buttons["Close"].waitForExistence(timeout: 15))
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
        let folder = picker.staticTexts["PdfCraftMobilePOC"].firstMatch
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
        // Files can report an invalid accessibility hit point for a visible thumbnail.
        item.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        XCTAssertTrue(app.otherElements["Browse View (Picker)"].waitForNonExistence(timeout: 20),
                      app.debugDescription)
    }
}
