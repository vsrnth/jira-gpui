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
        for (key, expectedPriority) in [
            ("DESK-179", "Highest"),
            ("DESK-171", "High"),
            ("DESK-163", "Medium"),
            ("DESK-176", "Low"),
            ("DESK-184", "Lowest"),
        ] {
            let issueRow = try require(
                app.descendants(matching: .any)["issue-row-\(key)"],
                "issue-row-\(key)"
            )
            XCTAssertTrue(
                issueRow.title.contains("Priority: \(expectedPriority)")
                    || issueRow.label.contains("Priority: \(expectedPriority)"),
                "\(key) should expose its exact Jira priority semantically"
            )
        }
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

        let emptyDescription = try require(
            app.descendants(matching: .any)["issue-detail-description"],
            "issue-detail-description"
        )
        let emptyDescriptionSemanticText = [
            emptyDescription.label,
            emptyDescription.title,
            emptyDescription.value as? String ?? "",
        ].joined(separator: " ")
        XCTAssertTrue(
            emptyDescriptionSemanticText.contains("No description supplied."),
            "detail-loaded empty descriptions should expose explicit empty copy semantically"
        )
        let detailLoading = app.descendants(matching: .any).matching(
            NSPredicate(format: "label == %@", "Loading issue details…")
        ).firstMatch
        XCTAssertFalse(detailLoading.exists, "cached empty detail should not show a detail spinner")

        storyRow.click()
        row.click()
        let reselectedEmptyDescription = try require(
            app.descendants(matching: .any)["issue-detail-description"],
            "issue-detail-description after reselect"
        )
        let reselectedDescriptionSemanticText = [
            reselectedEmptyDescription.label,
            reselectedEmptyDescription.title,
            reselectedEmptyDescription.value as? String ?? "",
        ].joined(separator: " ")
        XCTAssertTrue(
            reselectedDescriptionSemanticText.contains("No description supplied."),
            "reselecting cached empty detail should retain explicit empty copy semantically"
        )
        XCTAssertFalse(detailLoading.exists, "reselecting cached empty detail should remain spinner-free")

        let details = try require(
            app.descendants(matching: .any)["issue-detail-details"],
            "issue-detail-details"
        )
        let detailsButton = try require(
            app.buttons["issue-detail-details-trigger"],
            "issue-detail-details-trigger"
        )
        XCTAssertEqual(
            detailsButton.label.isEmpty ? detailsButton.title : detailsButton.label,
            "Details"
        )
        let expandedHeight = details.frame.height
        XCTAssertGreaterThan(expandedHeight, 0, "expanded Details group should have a meaningful height")

        detailsButton.click()
        let collapseExpectation = XCTNSPredicateExpectation(
            predicate: NSPredicate { object, _ in
                guard let element = object as? XCUIElement else {
                    return false
                }
                return element.frame.height <= expandedHeight - 4
            },
            object: details
        )
        XCTAssertTrue(XCTWaiter.wait(for: [collapseExpectation], timeout: 8) == .completed)
        let collapsedHeight = details.frame.height
        XCTAssertLessThan(
            collapsedHeight,
            expandedHeight - 4,
            "collapsing Details should materially reduce its height"
        )
        XCTAssertTrue(detailsButton.exists, "Details trigger should remain discoverable when collapsed")

        detailsButton.click()
        let reopenExpectation = XCTNSPredicateExpectation(
            predicate: NSPredicate { object, _ in
                guard let element = object as? XCUIElement else {
                    return false
                }
                return element.frame.height >= expandedHeight - 4
            },
            object: details
        )
        XCTAssertTrue(XCTWaiter.wait(for: [reopenExpectation], timeout: 8) == .completed)
        XCTAssertLessThanOrEqual(
            abs(details.frame.height - expandedHeight),
            4,
            "reopening Details should restore its expanded height"
        )
        XCTAssertTrue(detailsButton.exists, "Details trigger should remain discoverable when reopened")
    }

    func testRichContent() throws {
        try launchFixture(scenario: "rich-content")
        let markdownSurface = try require(
            app.descendants(matching: .any)["issue-description-markdown"],
            "issue-description-markdown"
        )
        XCTAssertGreaterThan(markdownSurface.frame.width, 0, "Markdown description should have a visible width")
        XCTAssertGreaterThan(markdownSurface.frame.height, 80, "Markdown description should lay out multiple rendered lines")
        let renderedParent = app.staticTexts.matching(
            NSPredicate(format: "value == %@", "Parent")
        ).firstMatch
        XCTAssertTrue(renderedParent.exists, "Markdown heading should expose its rendered text semantically")
        let renderedProblem = app.staticTexts.matching(
            NSPredicate(format: "value == %@", "Problem")
        ).firstMatch
        XCTAssertTrue(renderedProblem.exists, "Markdown body should expose the rendered Problem heading semantically")
        let rawHeading = app.staticTexts.matching(
            NSPredicate(format: "value CONTAINS %@", "## Parent")
        ).firstMatch
        XCTAssertFalse(rawHeading.exists, "Markdown heading delimiters must not be exposed as visible text")
        let rawCode = app.staticTexts.matching(
            NSPredicate(format: "value CONTAINS %@", "`UploadedRecordingsProcessing")
        ).firstMatch
        XCTAssertFalse(rawCode.exists, "Markdown inline-code delimiters must not be exposed as visible text")
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
        let emptyCell = try require(
            app.descendants(matching: .any)["rich-text-table-cell-2-0"],
            "rich-text-table-cell-2-0"
        )
        let secondEmptyCell = try require(
            app.descendants(matching: .any)["rich-text-table-cell-2-1"],
            "rich-text-table-cell-2-1"
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
        XCTAssertGreaterThan(emptyCell.frame.height, 0, "empty table cells should retain a bounded cell surface")
        XCTAssertGreaterThan(secondEmptyCell.frame.height, 0, "second empty table cell should retain a bounded cell surface")
        XCTAssertLessThanOrEqual(
            abs(emptyCell.frame.minY - secondEmptyCell.frame.minY),
            2.0,
            "empty table cells should share a top edge"
        )
        XCTAssertLessThanOrEqual(
            abs(emptyCell.frame.maxY - secondEmptyCell.frame.maxY),
            2.0,
            "empty table cells should share a bottom edge"
        )
        XCTAssertEqual(emptyCell.label, "", "first empty cell should have no visible fallback text")
        XCTAssertEqual(secondEmptyCell.label, "", "second empty cell should have no visible fallback text")
        XCTAssertEqual(emptyCell.value as? String ?? "", "", "first empty cell should remain blank")
        XCTAssertEqual(secondEmptyCell.value as? String ?? "", "", "second empty cell should remain blank")
        _ = try require(app.descendants(matching: .any)["rich-image-fixture-image"], "rich-image-fixture-image")
        _ = try require(
            app.descendants(matching: .any)["rich-image-comment-fixture-image"],
            "rich-image-comment-fixture-image"
        )

        let loading = app.descendants(matching: .any).matching(
            NSPredicate(format: "label == %@", "Loading image…")
        ).firstMatch
        XCTAssertFalse(loading.exists, "preloaded fixture image must not regress to a loading spinner")
        let unsupported = app.descendants(matching: .any).matching(
            NSPredicate(format: "label == %@", "Some Jira content isn't supported yet.")
        ).firstMatch
        XCTAssertFalse(unsupported.exists, "valid rich content must not show the unsupported sentinel")

        let statusNodes = app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "rich-text-status-")
        )
        XCTAssertEqual(statusNodes.count, 2, "fixture should expose one status node per result")
        for expected in ["Pass", "Fail"] {
            let status = try require(
                statusNodes.matching(NSPredicate(format: "value == %@", expected)).firstMatch,
                "rich-text-status-\(expected.lowercased())"
            )
            XCTAssertEqual(
                status.value as? String,
                expected,
                "status \(expected) should expose its exact AX value"
            )
        }

        let taskItems = app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "rich-text-task-item-")
        )
        XCTAssertEqual(taskItems.count, 2, "fixture table cell should expose TODO and DONE task items")
        let todoTask = try require(
            taskItems.matching(NSPredicate(format: "value == %@", "Todo task")).firstMatch,
            "Todo task item"
        )
        let doneTask = try require(
            taskItems.matching(NSPredicate(format: "value == %@", "Done task")).firstMatch,
            "Done task item"
        )
        XCTAssertEqual(todoTask.value as? String, "Todo task", "TODO state should remain semantic")
        XCTAssertEqual(doneTask.value as? String, "Done task", "DONE state should remain semantic")

        let decisionItems = app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "rich-text-decision-item-")
        )
        XCTAssertEqual(decisionItems.count, 2, "fixture should expose decided and undecided decisions")
        let decided = try require(
            decisionItems.matching(NSPredicate(format: "value == %@", "Decided decision")).firstMatch,
            "Decided decision item"
        )
        let undecided = try require(
            decisionItems.matching(NSPredicate(format: "value == %@", "Undecided decision")).firstMatch,
            "Undecided decision item"
        )
        XCTAssertEqual(decided.value as? String, "Decided decision")
        XCTAssertEqual(undecided.value as? String, "Undecided decision")

        let expand = try require(
            app.descendants(matching: .any).matching(
                NSPredicate(format: "identifier BEGINSWITH %@ AND label == %@", "rich-text-expand-", "Details")
            ).firstMatch,
            "Details expand"
        )
        let nestedExpand = try require(
            app.descendants(matching: .any).matching(
                NSPredicate(format: "identifier BEGINSWITH %@ AND label == %@", "rich-text-nested-expand-", "More details")
            ).firstMatch,
            "More details expand"
        )
        XCTAssertEqual(expand.value as? String, "Expanded")
        XCTAssertEqual(nestedExpand.value as? String, "Expanded")
        XCTAssertEqual(expand.label.isEmpty ? expand.title : expand.label, "Details")
        XCTAssertEqual(nestedExpand.label.isEmpty ? nestedExpand.title : nestedExpand.label, "More details")

        let emojiDate = try require(
            app.descendants(matching: .any).matching(
                NSPredicate(format: "identifier BEGINSWITH %@ AND value == %@", "rich-text-paragraph-", "✅ 2026-08-30")
            ).firstMatch,
            "emoji/date paragraph"
        )
        XCTAssertTrue(
            emojiDate.identifier.hasPrefix("rich-text-paragraph-"),
            "emoji/date line should use a rich-text paragraph accessibility ID"
        )
        XCTAssertEqual(emojiDate.value as? String, "✅ 2026-08-30")

        for expected in [
            "Epic: ENG-43",
            "Per the ENG-43, after",
            "OPS-7",
        ] {
            let paragraph = try require(
                app.staticTexts.matching(NSPredicate(format: "value == %@", expected)).firstMatch,
                "rich paragraph (expected)"
            )
            XCTAssertEqual(
                paragraph.value as? String,
                expected,
                "rich paragraph should expose exact value"
            )
        }

        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "rich-content-final"
        screenshot.lifetime = .keepAlways
        add(screenshot)
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
        let collapsedWorkspaceHeader = try require(
            app.descendants(matching: .any)["sidebar-workspace-header"],
            "sidebar-workspace-header after collapse"
        )
        let collapsedToggle = try require(
            app.descendants(matching: .any)["sidebar-toggle"],
            "sidebar-toggle after collapse"
        )
        XCTAssertTrue(
            sidebar.frame.contains(collapsedWorkspaceHeader.frame),
            "collapsed workspace header should remain inside the sidebar rail"
        )
        XCTAssertTrue(
            sidebar.frame.contains(collapsedToggle.frame),
            "collapsed sidebar toggle should remain inside the sidebar rail"
        )
        XCTAssertLessThanOrEqual(
            abs(collapsedWorkspaceHeader.frame.midX - collapsedToggle.frame.midX),
            2.0,
            "collapsed workspace header and toggle should share a center line"
        )
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
