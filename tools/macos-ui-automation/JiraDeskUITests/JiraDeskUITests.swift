import XCTest

final class JiraDeskUITests: XCTestCase {
    private var app: XCUIApplication!

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    override func tearDownWithError() throws {
        if (testRun?.totalFailureCount ?? 0) > 0, let app {
            let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
            screenshot.name = "failure-window"
            screenshot.lifetime = .keepAlways
            add(screenshot)

            let tree = XCTAttachment(string: redactedDebugDescription(for: app))
            tree.name = "failure-accessibility-tree"
            tree.lifetime = .keepAlways
            add(tree)
        }
        if let app, app.state != .notRunning {
            app.terminate()
        }
    }

    func testOnboarding() throws {
        try launchFixture(scenario: "onboarding")
        try step("Open Jira connection dialog") {
            let trigger = try require(app.descendants(matching: .any)["onboarding-connect-trigger"], "onboarding-connect-trigger")
            trigger.click()
        }

        try step("Enter non-secret fixture values") {
            try setValue("sample", identifier: "onboarding-jira-site")
            try setValue("ui-test@example.invalid", identifier: "onboarding-atlassian-email")
            _ = try require(app.descendants(matching: .any)["onboarding-api-token"], "onboarding-api-token")
            assertVisibleValue("sample", identifier: "onboarding-jira-site")
            assertVisibleValue("ui-test@example.invalid", identifier: "onboarding-atlassian-email")
        }

        try step("Keep fixture connection submit untouched") {
            _ = try require(app.descendants(matching: .any)["onboarding-connect-dialog-submit"], "onboarding-connect-dialog-submit")
        }

        try step("Cancel connection dialog") {
            let cancel = try require(app.descendants(matching: .any)["onboarding-connect-dialog-cancel"], "onboarding-connect-dialog-cancel")
            cancel.click()
            XCTAssertTrue(waitForAbsence(app.descendants(matching: .any)["onboarding-connect-dialog-submit"]))
        }
    }

    func testIssues() throws {
        try launchFixture(scenario: "issues")
        let storyRow = try require(app.descendants(matching: .any)["issue-row-DESK-184"], "issue-row-DESK-184")
        XCTAssertTrue(storyRow.title.contains("(Story)"), "known Story row should retain its issue-type identity")
        let row = try require(app.descendants(matching: .any)["issue-row-DESK-179"], "issue-row-DESK-179")
        XCTAssertTrue(row.title.contains("(Task)"), "known Task row should retain its issue-type identity")
        let workspaceHeader = try require(
            app.descendants(matching: .any)["sidebar-workspace-header"],
            "sidebar-workspace-header"
        )
        let workspaceIdentity = workspaceHeader.label.isEmpty ? workspaceHeader.title : workspaceHeader.label
        XCTAssertTrue(
            workspaceIdentity == "sample" || workspaceIdentity.hasPrefix("sample ·"),
            "fixture workspace should expose the normalized site slug in its identity"
        )
        row.click()
        let detail = try require(app.descendants(matching: .any)["issue-detail"], "issue-detail")
        let detailExpectation = XCTNSPredicateExpectation(
            predicate: NSPredicate { object, _ in
                guard let detail = object as? XCUIElement else {
                    return false
                }
                return detail.label == "Issue detail for DESK-179"
                    || detail.title == "Issue detail for DESK-179"
            },
            object: detail
        )
        XCTAssertTrue(XCTWaiter.wait(for: [detailExpectation], timeout: 8) == .completed)
    }

    func testRichContent() throws {
        try launchFixture(scenario: "rich-content")
        _ = try require(
            app.descendants(matching: .any)["rich-text-horizontal-rule"],
            "rich-text-horizontal-rule"
        )
        _ = try require(app.descendants(matching: .any)["rich-text-table"], "rich-text-table")
        let shortCell = try require(
            app.descendants(matching: .any)["rich-text-table-cell-1-0"],
            "rich-text-table-cell-1-0"
        )
        let multilineCell = try require(
            app.descendants(matching: .any)["rich-text-table-cell-1-1"],
            "rich-text-table-cell-1-1"
        )
        XCTAssertLessThanOrEqual(
            abs(shortCell.frame.minY - multilineCell.frame.minY),
            2.0,
            "uneven table cells should share a top edge"
        )
        XCTAssertLessThanOrEqual(
            abs(shortCell.frame.maxY - multilineCell.frame.maxY),
            2.0,
            "uneven table cells should share a bottom edge"
        )
        _ = try require(app.descendants(matching: .any)["rich-image-fixture-image"], "rich-image-fixture-image")

        let loading = app.descendants(matching: .any).matching(
            NSPredicate(format: "label == %@", "Loading image…")
        ).firstMatch
        XCTAssertFalse(loading.exists, "preloaded fixture image must not regress to a loading spinner")
        let unsupported = app.descendants(matching: .any).matching(
            NSPredicate(format: "label == %@", "Some Jira content isn't supported yet.")
        ).firstMatch
        XCTAssertFalse(unsupported.exists, "valid rich content must not show the unsupported sentinel")
        for (identifier, expected) in [
            ("rich-text-paragraph-0", "Epic: ENG-43"),
            ("rich-text-paragraph-1", "Per the ENG-43, after"),
            ("rich-text-paragraph-2", "OPS-7"),
        ] {
            let paragraph = try require(app.staticTexts[identifier], identifier)
            XCTAssertEqual(
                paragraph.value as? String,
                expected,
                "\(identifier) should expose exact rich paragraph value"
            )
        }
    }

    func testSettings() throws {
        try launchFixture(scenario: "settings")
        let sidebarMenu = try require(app.descendants(matching: .any)["nav-settings"], "nav-settings")

        let sidebar = try require(app.descendants(matching: .any)["dashboard-sidebar"], "dashboard-sidebar")
        let settingsIdentity = sidebarMenu.label.isEmpty ? sidebarMenu.title : sidebarMenu.label
        XCTAssertEqual(settingsIdentity, "Settings")
        let settingsFrame = sidebarMenu.frame
        XCTAssertTrue(sidebar.frame.contains(settingsFrame), "expanded Settings navigation must remain inside the sidebar bounds")
        XCTAssertGreaterThanOrEqual(
            settingsFrame.width,
            200,
            "expanded Settings submenu needs enough width for Desktop notifications"
        )
        XCTAssertGreaterThanOrEqual(
            settingsFrame.height,
            180,
            "expanded Settings submenu needs enough height for Desktop notifications"
        )

        _ = try require(app.descendants(matching: .any)["appearance-dark"], "appearance-dark")
        let darkToggle = try require(app.checkBoxes["Use Dark appearance"], "Use Dark appearance")
        darkToggle.click()
        let darkExpectation = XCTNSPredicateExpectation(
            predicate: NSPredicate { object, _ in
                guard let checkbox = object as? XCUIElement else {
                    return false
                }
                if checkbox.isSelected {
                    return true
                }
                if let number = checkbox.value as? NSNumber {
                    return number.boolValue || number.intValue == 1
                }
                if let value = checkbox.value as? String {
                    return value == "1" || value.lowercased() == "true"
                }
                return false
            },
            object: darkToggle
        )
        XCTAssertTrue(XCTWaiter.wait(for: [darkExpectation], timeout: 8) == .completed)

        let toggle = try require(app.descendants(matching: .any)["sidebar-toggle"], "sidebar-toggle")
        toggle.click()
        XCTAssertFalse(sidebarMenu.frame.width > 100, "collapsed sidebar should use the icon-only rail")
    }

    func testUpdates() throws {
        try launchFixture(scenario: "updates")
        let updates = try require(app.descendants(matching: .any)["nav-updates"], "nav-updates")
        updates.click()
        _ = try require(app.descendants(matching: .any)["update-list"], "update-list")
        let dot = try require(app.descendants(matching: .any)["update-unread-dot-0"], "update-unread-dot-0")
        let metadata = try require(app.descendants(matching: .any)["update-metadata-0"], "update-metadata-0")
        let dotCenter = CGPoint(x: dot.frame.midX, y: dot.frame.midY)
        let metadataCenter = CGPoint(x: metadata.frame.midX, y: metadata.frame.midY)
        XCTAssertLessThanOrEqual(abs(dotCenter.y - metadataCenter.y), 2.0, "unread dot should align with first metadata line")
    }

    func testTeam() throws {
        try launchFixture(scenario: "team")
        let team = try require(app.descendants(matching: .any)["nav-team"], "nav-team")
        team.click()
        _ = try require(app.descendants(matching: .any)["team-table"], "team-table")
    }

    private func launchFixture(scenario: String) throws {
        let productsDirectory = Bundle(for: JiraDeskUITests.self).bundleURL
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let appURL = productsDirectory.appendingPathComponent("Jira Desk UI Automation.app", isDirectory: true)
        let dataURL = productsDirectory.appendingPathComponent("Jira Desk UI Automation Data", isDirectory: true)
        let stateURL = productsDirectory.appendingPathComponent("Jira Desk UI Automation State", isDirectory: true)
        guard FileManager.default.fileExists(atPath: appURL.path) else {
            XCTFail("Fixture app is missing beside the XCTest runner: \(appURL.path)")
            throw NSError(domain: "JiraDeskUITests", code: 2)
        }
        guard FileManager.default.fileExists(atPath: dataURL.path),
              FileManager.default.fileExists(atPath: stateURL.path) else {
            XCTFail("Fixture data roots are missing beside the XCTest runner")
            throw NSError(domain: "JiraDeskUITests", code: 3)
        }
        app = XCUIApplication(url: appURL)
        app.launchArguments = ["--scenario", scenario]
        app.launchEnvironment["XDG_DATA_HOME"] = dataURL.path
        app.launchEnvironment["XDG_STATE_HOME"] = stateURL.path
        app.launch()
        guard app.wait(for: .runningForeground, timeout: 8) else {
            XCTFail("Jira Desk UI Automation did not reach the foreground")
            throw NSError(domain: "JiraDeskUITests", code: 4)
        }
    }

    private func setValue(_ value: String, identifier: String) throws {
        let field = try require(app.descendants(matching: .any)[identifier], identifier)
        field.click()
        var prefix = ""
        for character in value {
            let character = String(character)
            let expected = prefix + character
            var acknowledged = false
            for _ in 0..<3 {
                app.typeKey(character, modifierFlags: [])
                let expectation = XCTNSPredicateExpectation(
                    predicate: NSPredicate { object, _ in
                        guard let field = object as? XCUIElement else {
                            return false
                        }
                        return (field.value as? String) == expected
                    },
                    object: field
                )
                if XCTWaiter.wait(for: [expectation], timeout: 2) == .completed {
                    acknowledged = true
                    break
                }
            }
            guard acknowledged else {
                XCTFail("Fixture field did not acknowledge the next character: \(identifier)")
                throw NSError(domain: "JiraDeskUITests", code: 5)
            }
            prefix = expected
        }
    }

    private func assertVisibleValue(_ expected: String, identifier: String) {
        let field = app.descendants(matching: .any)[identifier]
        let valueExpectation = XCTNSPredicateExpectation(
            predicate: NSPredicate { object, _ in
                guard let field = object as? XCUIElement else {
                    return false
                }
                return (field.value as? String) == expected
            },
            object: field
        )
        // Keep this assertion boolean-only so XCTest never emits the input value.
        XCTAssertTrue(XCTWaiter.wait(for: [valueExpectation], timeout: 8) == .completed)
    }

    @discardableResult
    private func require(_ element: XCUIElement, _ identifier: String) throws -> XCUIElement {
        guard element.waitForExistence(timeout: 8) else {
            XCTFail("Missing accessibility node: \(identifier)")
            throw NSError(domain: "JiraDeskUITests", code: 1)
        }
        return element
    }

    private func waitForAbsence(_ element: XCUIElement) -> Bool {
        let expectation = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "exists == false"),
            object: element
        )
        return XCTWaiter.wait(for: [expectation], timeout: 8) == .completed
    }

    private func redactedDebugDescription(for app: XCUIApplication) -> String {
        ["sample", "ui-test@example.invalid"].reduce(app.debugDescription) {
            $0.replacingOccurrences(of: $1, with: "<fixture-value-redacted>")
        }
    }

    private func step(_ name: String, _ body: () throws -> Void) rethrows {
        try XCTContext.runActivity(named: name) { _ in
            try body()
        }
    }
}
