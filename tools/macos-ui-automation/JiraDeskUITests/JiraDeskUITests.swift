import XCTest
import AppKit

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
        let hostWindow = try require(app.windows.firstMatch, "fixture host window")
        let intro = try require(
            app.descendants(matching: .any)["onboarding-intro"],
            "onboarding-intro"
        )
        XCTAssertTrue(
            semanticText(intro).contains(
                "See issues assigned to or watched by your Jira account. Have your Jira Cloud site, Atlassian email, and scoped API token ready."
            ),
            "onboarding intro should expose the complete connection guidance"
        )
        assertFiniteBounded(intro.frame, name: "onboarding-intro")
        XCTAssertTrue(hostWindow.frame.contains(intro.frame), "onboarding intro should remain inside the host window")

        let trigger = try require(
            app.descendants(matching: .any)["onboarding-connect-trigger"],
            "onboarding-connect-trigger"
        )
        assertFiniteBounded(trigger.frame, name: "onboarding-connect-trigger")
        XCTAssertGreaterThanOrEqual(trigger.frame.height, 44, "connection trigger should retain a 44 px target")
        XCTAssertLessThanOrEqual(trigger.frame.height, 100, "connection trigger height should remain sensible")
        XCTAssertLessThan(trigger.frame.width, 600, "connection trigger width should remain sensible")
        XCTAssertTrue(hostWindow.frame.contains(trigger.frame), "connection trigger should remain inside the host window")

        try step("Open Jira connection dialog") {
            trigger.click()
        }

        let dialogBodies = app.descendants(matching: .any)
            .matching(identifier: "onboarding-connect-dialog-body")
        let body = try require(dialogBodies.firstMatch, "onboarding-connect-dialog-body")
        XCTAssertEqual(dialogBodies.count, 1, "the Base Root should host one connection dialog body")
        assertFiniteBounded(body.frame, name: "onboarding-connect-dialog-body")
        XCTAssertTrue(hostWindow.frame.contains(body.frame), "connection dialog body should remain inside the host window")

        let site = try require(app.descendants(matching: .any)["onboarding-jira-site"], "onboarding-jira-site")
        let email = try require(
            app.descendants(matching: .any)["onboarding-atlassian-email"],
            "onboarding-atlassian-email"
        )
        let token = try require(app.descendants(matching: .any)["onboarding-api-token"], "onboarding-api-token")
        let remember = try require(app.descendants(matching: .any)["remember-jira-login"], "remember-jira-login")
        let cancelControls = app.descendants(matching: .any)
            .matching(identifier: "onboarding-connect-dialog-cancel")
        let cancel = try require(cancelControls.firstMatch, "onboarding-connect-dialog-cancel")
        XCTAssertEqual(cancelControls.count, 1, "the dialog should expose one Cancel control")
        let submitControls = app.descendants(matching: .any)
            .matching(identifier: "onboarding-connect-dialog-submit")
        let submit = try require(submitControls.firstMatch, "onboarding-connect-dialog-submit")
        XCTAssertEqual(submitControls.count, 1, "the dialog should expose one Connect control")
        for (control, identifier) in [
            (site, "onboarding-jira-site"),
            (email, "onboarding-atlassian-email"),
            (token, "onboarding-api-token"),
        ] {
            assertFiniteBounded(control.frame, name: identifier)
            XCTAssertGreaterThan(control.frame.width, 200, "onboarding field should have bounded width: \(identifier)")
            XCTAssertGreaterThan(control.frame.height, 20, "onboarding field should have bounded height: \(identifier)")
            XCTAssertTrue(body.frame.contains(control.frame), "control should remain inside the dialog body: \(identifier)")
        }
        assertFiniteBounded(remember.frame, name: "remember-jira-login")
        XCTAssertGreaterThan(remember.frame.width, 180, "remember control should have bounded width")
        XCTAssertGreaterThan(remember.frame.height, 12, "remember control should have bounded height")
        XCTAssertTrue(body.frame.contains(remember.frame), "remember control should remain inside the dialog body")
        for (control, identifier) in [
            (cancel, "onboarding-connect-dialog-cancel"),
            (submit, "onboarding-connect-dialog-submit"),
        ] {
            assertFiniteBounded(control.frame, name: identifier)
            XCTAssertGreaterThan(control.frame.width, 80, "dialog action should have bounded width: \(identifier)")
            XCTAssertGreaterThan(control.frame.height, 20, "dialog action should have bounded height: \(identifier)")
            XCTAssertTrue(hostWindow.frame.contains(control.frame), "dialog action should remain inside the host window: \(identifier)")
        }

        let siteGuidanceCopy = "Enter your-team or your-team.atlassian.net (HTTPS)."
        let siteGuidance = try require(
            app.descendants(matching: .any)["onboarding-jira-site-help"],
            "onboarding-jira-site-help"
        )
        XCTAssertTrue(semanticText(siteGuidance).contains(siteGuidanceCopy))
        assertFiniteBounded(siteGuidance.frame, name: "Jira site guidance")
        XCTAssertTrue(body.frame.contains(siteGuidance.frame), "Jira site guidance should remain inside the dialog body")

        try step("Enter non-secret fixture values") {
            try setValue("sample", identifier: "onboarding-jira-site")
            try setValue("ui-test@example.invalid", identifier: "onboarding-atlassian-email")
            assertVisibleValue("sample", identifier: "onboarding-jira-site")
            assertVisibleValue("ui-test@example.invalid", identifier: "onboarding-atlassian-email")
        }

        try step("Keep fixture connection submit untouched") {
            XCTAssertTrue(submit.exists)
        }

        try step("Cancel connection dialog") {
            cancel.click()
            XCTAssertTrue(waitForAbsence(app.descendants(matching: .any)["onboarding-connect-dialog-submit"]))
        }
    }

    func testOnboardingBusy() throws {
        try launchFixture(scenario: "onboarding-busy")

        let body = try require(
            app.descendants(matching: .any)["onboarding-connect-dialog-body"],
            "onboarding-connect-dialog-body"
        )
        let status = try require(
            app.descendants(matching: .any)["onboarding-status"],
            "onboarding-status"
        )
        let statusSemanticText = [status.label, status.title, status.value as? String ?? ""]
            .joined(separator: " ")
        XCTAssertTrue(
            statusSemanticText.contains("Verifying Jira credentials and configuring your Jira connection…"),
            "busy onboarding should expose the stable verification progress copy"
        )
        XCTAssertTrue(body.frame.contains(status.frame), "progress status should be inside the dialog body")
        XCTAssertGreaterThan(status.frame.width, 240, "progress status should have a meaningful bounded width")
        XCTAssertGreaterThan(status.frame.height, 20, "progress status should have a meaningful bounded height")
        // Spinner is visual-only; its accessible progress semantics intentionally live on the
        // parent Status node, which keeps the status copy stable even when AX merges children.

        let site = try require(
            app.descendants(matching: .any)["onboarding-jira-site"],
            "onboarding-jira-site"
        )
        let email = try require(
            app.descendants(matching: .any)["onboarding-atlassian-email"],
            "onboarding-atlassian-email"
        )
        let token = try require(
            app.descendants(matching: .any)["onboarding-api-token"],
            "onboarding-api-token"
        )
        let remember = try require(
            app.descendants(matching: .any)["remember-jira-login"],
            "remember-jira-login"
        )
        let initialSite = axValue(site)
        let initialEmail = axValue(email)
        let initialToken = axValue(token)
        let initialRemember = axValue(remember)

        for (control, identifier) in [
            (site, "onboarding-jira-site"),
            (email, "onboarding-atlassian-email"),
            (token, "onboarding-api-token"),
        ] {
            XCTAssertTrue(body.frame.contains(control.frame), "control should remain inside the dialog body: \(identifier)")
            XCTAssertGreaterThan(control.frame.width, 200, "busy onboarding field should have bounded width: \(identifier)")
            XCTAssertGreaterThan(control.frame.height, 20, "busy onboarding field should have bounded height: \(identifier)")
            control.click()
            app.typeKey("x", modifierFlags: [])
        }
        XCTAssertEqual(axValue(site), initialSite, "busy Jira site field must not mutate")
        XCTAssertEqual(axValue(email), initialEmail, "busy Atlassian email field must not mutate")
        XCTAssertEqual(axValue(token), initialToken, "busy API token field must not mutate")

        XCTAssertTrue(body.frame.contains(remember.frame), "remember control should remain inside the dialog body")
        XCTAssertGreaterThan(remember.frame.width, 180, "remember control should have bounded width")
        XCTAssertGreaterThan(remember.frame.height, 12, "remember control should have bounded height")
        remember.click()
        XCTAssertEqual(axValue(remember), initialRemember, "busy remember control must not mutate")

        let assertBusyDialogStillPresent = {
            let currentStatus = self.app.descendants(matching: .any)["onboarding-status"]
            XCTAssertTrue(currentStatus.exists, "busy status must remain after an inert action")
            let currentStatusText = [currentStatus.label, currentStatus.title, self.axValue(currentStatus)]
                .joined(separator: " ")
            XCTAssertTrue(
                currentStatusText.contains("Verifying Jira credentials and configuring your Jira connection…"),
                "busy status copy must remain after an inert action"
            )
            XCTAssertTrue(body.exists, "busy connection dialog must remain after an inert action")
        }

        let cancel = try require(
            app.descendants(matching: .any)["onboarding-connect-dialog-cancel"],
            "onboarding-connect-dialog-cancel"
        )
        XCTAssertGreaterThan(cancel.frame.width, 80, "busy Cancel action should have bounded width")
        XCTAssertGreaterThan(cancel.frame.height, 20, "busy Cancel action should have bounded height")
        cancel.click()
        assertBusyDialogStillPresent()

        let submit = try require(
            app.descendants(matching: .any)["onboarding-connect-dialog-submit"],
            "onboarding-connect-dialog-submit"
        )
        XCTAssertGreaterThan(submit.frame.width, 80, "busy submit action should have bounded width")
        XCTAssertGreaterThan(submit.frame.height, 20, "busy submit action should have bounded height")
        submit.click()
        assertBusyDialogStillPresent()

        XCTAssertFalse(
            app.descendants(matching: .any)["dashboard-sidebar"].exists,
            "busy inert actions must not open a dashboard"
        )

        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "onboarding-busy-final"
        screenshot.lifetime = .keepAlways
        add(screenshot)
    }

    private func axValue(_ element: XCUIElement) -> String {
        if let value = element.value as? String {
            return value
        }
        if let value = element.value as? NSNumber {
            return value.stringValue
        }
        return element.value.map { String(describing: $0) } ?? ""
    }

    func testIssueOverviewDetails() throws {
        try launchFixture(scenario: "issues")

        let hostWindow = try require(app.windows.firstMatch, "fixture host window")
        let toolbar = try require(app.descendants(matching: .any)["issues-toolbar"], "issues-toolbar")
        let table = try require(app.descendants(matching: .any)["issues-table"], "issues-table")
        XCTAssertTrue(hostWindow.frame.contains(toolbar.frame), "Issues toolbar should span the table and detail workspace")
        XCTAssertFalse(app.descendants(matching: .any)["issue-detail"].exists, "overview should start without an open detail pane")
        let overviewWidth = table.frame.width

        let firstRow = try require(app.descendants(matching: .any)["issue-row-DESK-179"], "issue-row-DESK-179")
        firstRow.click()
        let detail = try require(app.descendants(matching: .any)["issue-detail"], "issue-detail after selecting DESK-179")
        let splitTable = try require(app.descendants(matching: .any)["issues-table"], "issues-table beside selected detail")
        XCTAssertTrue(waitForSemanticText("Issue detail for DESK-179", in: detail), "selecting DESK-179 should open its detail")
        XCTAssertTrue(hostWindow.frame.contains(splitTable.frame), "table pane should remain inside the fixture window")
        XCTAssertGreaterThanOrEqual(splitTable.frame.width, 280, "table pane should retain a usable viewport")
        XCTAssertFalse(splitTable.frame.intersects(detail.frame), "table and detail panes should remain separate")
        XCTAssertLessThan(splitTable.frame.width, overviewWidth * 0.7, "detail should use a distinct pane beside the table")
        XCTAssertTrue(firstRow.isSelected, "selected issue row should expose selected semantics")
        XCTAssertLessThan(firstRow.frame.height, 45, "selected table row should remain compact")

        let secondRow = try require(app.descendants(matching: .any)["issue-row-DESK-171"], "issue-row-DESK-171 beside detail")
        secondRow.click()
        XCTAssertTrue(waitForSemanticText("Issue detail for DESK-171", in: detail), "selecting another row should update the existing detail pane")
        XCTAssertTrue(secondRow.isSelected, "the new issue row should expose selected semantics")

        let updatedHeader = try require(app.descendants(matching: .any)["issues-column-updated"], "Updated column header")
        let updatedCell = try require(app.descendants(matching: .any)["issue-cell-updated-DESK-171"], "DESK-171 Updated cell")
        splitTable.scroll(byDeltaX: 600, deltaY: 0)
        XCTAssertTrue(splitTable.frame.contains(updatedHeader.frame), "horizontal scroll should reveal the Updated header in the table viewport")
        XCTAssertTrue(splitTable.frame.contains(updatedCell.frame), "the matching Updated cell should scroll with its header")
        XCTAssertLessThanOrEqual(abs(updatedHeader.frame.minX - updatedCell.frame.minX), 12, "Updated cell should remain aligned with its header")
        splitTable.scroll(byDeltaX: -600, deltaY: 0)

        try setValue("MVP", identifier: "issue-search")
        let count = try require(app.descendants(matching: .any)["issue-list-summary"], "issue-list-summary after local filter")
        XCTAssertTrue(waitForSemanticText("1 of 5 Jira issues", in: count), "local summary filter should retain the total count while details stay open")
        XCTAssertTrue(waitForSemanticText("Issue detail for DESK-171", in: detail), "filtering the table should preserve the selected detail")
        let clear = try require(app.buttons["issue-filters-clear"], "issue-filters-clear while details are open")
        clear.click()
        XCTAssertTrue(waitForSemanticText("5 Jira issues", in: count), "clearing the filter should restore all fixture rows")

        let back = try require(app.buttons["issue-detail-back"], "issue-detail-back")
        back.click()
        let restoredTable = try require(app.descendants(matching: .any)["issues-table"], "issues-table after closing details")
        XCTAssertFalse(app.descendants(matching: .any)["issue-detail"].exists, "Back to issues should close the detail pane")
        XCTAssertGreaterThan(restoredTable.frame.width, overviewWidth * 0.85, "closing details should restore the full-width table")
    }

    func testIssues() throws {
        try launchFixture(scenario: "issues")

        let issueSearch = try require(app.descendants(matching: .any)["issue-search"], "issue-search")
        let hostWindow = try require(app.windows.firstMatch, "fixture host window")
        XCTAssertGreaterThan(issueSearch.frame.width, 220, "issue search should have enough room for a useful query")
        XCTAssertGreaterThan(issueSearch.frame.height, 20, "issue search should have a visible control height")
        XCTAssertTrue(hostWindow.frame.contains(issueSearch.frame), "issue search should stay inside the host window")
        XCTAssertFalse(app.descendants(matching: .any)["issue-search-submit"].exists, "Find key should remain contextual while the search is empty")

        let issuesTable = try require(app.descendants(matching: .any)["issues-table"], "issues-table")
        let issueListSummary = try require(app.descendants(matching: .any)["issue-list-summary"], "issue-list-summary")
        XCTAssertFalse(app.descendants(matching: .any)["issue-detail"].exists, "initial Issues should show the full overview without an open detail pane")
        XCTAssertGreaterThan(issuesTable.frame.width, hostWindow.frame.width * 0.65, "initial overview table should use the available content width")
        XCTAssertTrue(hostWindow.frame.contains(issuesTable.frame), "issues table should remain inside the host window")
        let toolbar = try require(app.descendants(matching: .any)["issues-toolbar"], "issues-toolbar")
        XCTAssertTrue(hostWindow.frame.contains(toolbar.frame), "issues toolbar should remain inside the host window")
        let title = try require(app.descendants(matching: .any)["issues-title"], "issues-title")
        XCTAssertTrue(semanticText(title).contains("Issues"), "overview should expose its Issues title")
        XCTAssertTrue(toolbar.frame.contains(title.frame), "Issues title should be inside the single toolbar")
        XCTAssertTrue(toolbar.frame.contains(issueSearch.frame), "search should be inside the single toolbar")
        let statusFilter = try require(app.descendants(matching: .any)["issue-status-filter"], "issue-status-filter")
        XCTAssertTrue(toolbar.frame.contains(statusFilter.frame), "status filter should be inside the single toolbar")
        XCTAssertTrue(semanticText(statusFilter).contains("Filter"), "status selector should expose the Filter label")
        XCTAssertTrue(semanticText(statusFilter).contains("All statuses"), "status selector should expose its current selection")
        XCTAssertFalse(issueSearch.frame.intersects(statusFilter.frame), "search and status selector must not overlap")
        let initialOverviewTableWidth = issuesTable.frame.width

        for column in ["key", "summary", "status", "assignee", "updated"] {
            let header = try require(
                app.descendants(matching: .any)["issues-column-\(column)"],
                "issues-column-\(column)"
            )
            assertFiniteBounded(header.frame, name: "issues-column-\(column)")
            XCTAssertTrue(issuesTable.frame.contains(header.frame), "\(column) header should be inside the issues table")
        }
        let columnHeaders = ["key", "summary", "status", "assignee", "updated"].map {
            app.descendants(matching: .any)["issues-column-\($0)"]
        }
        for index in 1..<columnHeaders.count {
            XCTAssertGreaterThanOrEqual(
                columnHeaders[index].frame.minX,
                columnHeaders[index - 1].frame.minX,
                "table columns should remain in their displayed left-to-right order"
            )
        }
        for (key, summary) in [
            ("DESK-184", "Surface Jira update notifications in the desktop feed"),
            ("DESK-179", "Package the Wayland build as an AppImage"),
            ("DESK-176", "Reconcile issues removed from a saved user set"),
            ("DESK-171", "Read-only Jira desktop MVP"),
            ("DESK-163", "Model pagination cursors from enhanced search"),
        ] {
            let row = try require(app.descendants(matching: .any)["issue-row-\(key)"], "issue-row-\(key)")
            assertFiniteBounded(row.frame, name: "issue-row-\(key)")
            XCTAssertGreaterThan(row.frame.height, 30, "table row should remain readable")
            XCTAssertLessThan(row.frame.height, 70, "table row should stay compact")
            XCTAssertTrue(issuesTable.frame.contains(row.frame), "\(key) row should be inside the issues table")
            let keyCell = try require(app.descendants(matching: .any)["issue-cell-key-\(key)"], "issue-cell-key-\(key)")
            let summaryCell = try require(app.descendants(matching: .any)["issue-cell-summary-\(key)"], "issue-cell-summary-\(key)")
            let statusCell = try require(app.descendants(matching: .any)["issue-cell-status-\(key)"], "issue-cell-status-\(key)")
            let assigneeCell = try require(app.descendants(matching: .any)["issue-cell-assignee-\(key)"], "issue-cell-assignee-\(key)")
            let updatedCell = try require(app.descendants(matching: .any)["issue-cell-updated-\(key)"], "issue-cell-updated-\(key)")
            let rowCells = [keyCell, summaryCell, statusCell, assigneeCell, updatedCell]
            for (index, cell) in rowCells.enumerated() {
                assertFiniteBounded(cell.frame, name: "issue cell \(index) for \(key)")
                XCTAssertTrue(row.frame.contains(cell.frame), "cell \(index) for \(key) should remain inside its row")
                XCTAssertLessThanOrEqual(abs(cell.frame.midY - row.frame.midY), 4, "cells for \(key) should align on one row")
                XCTAssertLessThanOrEqual(
                    abs(cell.frame.minX - columnHeaders[index].frame.minX),
                    12,
                    "cell \(index) for \(key) should align with its column header"
                )
                XCTAssertFalse(semanticText(cell).trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, "cell \(index) for \(key) should expose visible content semantically")
            }
            XCTAssertTrue(semanticText(keyCell).contains(key), "key cell should expose the issue key")
            XCTAssertGreaterThan(summaryCell.frame.width, 160, "summary cell should keep a useful visible text budget")
            XCTAssertLessThan(summaryCell.frame.width, 520, "summary cell should remain bounded so long summaries can truncate visually")
            XCTAssertTrue(semanticText(summaryCell).contains(summary), "summary cell should preserve full text when visual content is truncated")
        }

        let selectedRow = try require(app.descendants(matching: .any)["issue-row-DESK-184"], "selected issue row DESK-184")
        let unselectedRow = try require(app.descendants(matching: .any)["issue-row-DESK-171"], "unselected issue row DESK-171")
        XCTAssertTrue(selectedRow.isSelected, "the retained issue selection should have selected row semantics")
        XCTAssertFalse(unselectedRow.isSelected, "neighboring rows should remain unselected")
        let overviewScreenshot = hostWindow.screenshot()
        let selectedInterior = try requireScreenshotColor(
            overviewScreenshot,
            at: CGPoint(x: selectedRow.frame.maxX - 14, y: selectedRow.frame.midY),
            in: hostWindow,
            name: "selected overview row interior"
        )
        let unselectedInterior = try requireScreenshotColor(
            overviewScreenshot,
            at: CGPoint(x: unselectedRow.frame.maxX - 14, y: unselectedRow.frame.midY),
            in: hostWindow,
            name: "unselected overview row interior"
        )
        XCTAssertGreaterThan(
            rgbDistance(selectedInterior, unselectedInterior),
            18,
            "selected overview row should use the full blue row surface"
        )

        // Sidebar navigation replaces the removed workspace shortcut while keeping its destination coverage.
        let navIssuesMatches = app.descendants(matching: .any)
            .matching(identifier: "nav-issues")
        let navUpdates = try require(app.descendants(matching: .any)["nav-updates"], "nav-updates / Local updates")
        XCTAssertTrue(semanticText(navUpdates).contains("Local updates"), "sidebar should label the updates destination Local updates")
        navUpdates.click()
        _ = try require(app.descendants(matching: .any)["update-list"], "update-list from sidebar navigation")
        let navIssues = try require(navIssuesMatches.firstMatch, "nav-issues from Local updates")
        XCTAssertEqual(navIssuesMatches.count, 1, "the sidebar should expose one Issues navigation item")
        assertFiniteBounded(navIssues.frame, name: "nav-issues before returning")
        XCTAssertTrue(hostWindow.frame.contains(navIssues.frame), "Issues navigation should remain inside the host window")
        navIssues.click()
        let returnedNavIssues = try require(navIssuesMatches.firstMatch, "nav-issues after returning")
        assertFiniteBounded(returnedNavIssues.frame, name: "nav-issues after returning")
        XCTAssertTrue(hostWindow.frame.contains(returnedNavIssues.frame), "selected Issues navigation should remain inside the host window")
        let returnedTable = try require(app.descendants(matching: .any)["issues-table"], "issues-table after returning from Local updates")
        XCTAssertTrue(hostWindow.frame.contains(returnedTable.frame), "returning to Issues should restore the overview table")
        XCTAssertFalse(app.descendants(matching: .any)["issue-detail"].exists, "sidebar return should restore overview without an open detail pane")
        let issuesOutlineRows = app.outlineRows.matching(
            NSPredicate(format: "label == %@", "Issues")
        )
        let issuesOutlineRow = try require(issuesOutlineRows.firstMatch, "Issues sidebar OutlineRow")
        XCTAssertEqual(issuesOutlineRows.count, 1, "the sidebar should expose one Issues OutlineRow")
        let selectedExpectation = XCTNSPredicateExpectation(
            predicate: NSPredicate { object, _ in
                (object as? XCUIElement)?.isSelected == true
            },
            object: issuesOutlineRow
        )
        XCTAssertEqual(
            XCTWaiter.wait(for: [selectedExpectation], timeout: 8),
            .completed,
            "the Issues OutlineRow should become selected after the workspace returns"
        )
        XCTAssertEqual(
            app.outlineRows.allElementsBoundByIndex.filter({ $0.isSelected }).count,
            1,
            "the sidebar should have exactly one selected OutlineRow"
        )
        // A natural-language summary is a local filter. Pressing Enter must not turn it into a
        // Jira lookup or surface an unavailable-workspace error.
        try setValue("MVP", identifier: "issue-search")
        XCTAssertFalse(app.descendants(matching: .any)["issue-search-submit"].exists, "natural language search should not offer Find key")
        issueSearch.click()
        app.typeKey(XCUIKeyboardKey.return, modifierFlags: [])
        XCTAssertFalse(
            app.descendants(matching: .any)["remote-lookup-error"].exists,
            "pressing Enter for a local summary must not show a Jira lookup error"
        )
        XCTAssertTrue(waitForSemanticText("1 of 5 Jira issues", in: issueListSummary), "summary filtering should expose the deterministic filtered count")
        let localSummaryRow = try require(
            app.descendants(matching: .any)["issue-row-DESK-171"],
            "issue-row-DESK-171 after local summary search"
        )
        XCTAssertTrue(
            [localSummaryRow.label, localSummaryRow.title, localSummaryRow.value as? String ?? ""]
                .joined(separator: " ")
                .contains("Read-only Jira desktop MVP"),
            "summary search should retain the matching local issue"
        )

        // Clear the local filter before exercising status filtering.
        let clearFilters = try require(app.buttons["issue-filters-clear"], "issue-filters-clear")
        clearFilters.click()
        XCTAssertEqual(axValue(issueSearch), "", "Clear filters should reset the search input")
        XCTAssertTrue(waitForSemanticText("5 Jira issues", in: issueListSummary), "Clear filters should restore all local issues")

        // The status trigger and its popover option expose stable semantic IDs.
        XCTAssertTrue(semanticText(statusFilter).contains("Filter status: All statuses"), "status filter should expose the initial All statuses label")
        XCTAssertGreaterThan(statusFilter.frame.width, 120, "status filter should have a readable trigger width")
        statusFilter.click()
        let inProgressOption = try require(
            app.descendants(matching: .any)["status-filter-option-in-progress"],
            "status-filter-option-in-progress"
        )
        inProgressOption.click()
        let activeStatus = try require(
            app.descendants(matching: .any)["issue-status-filter"],
            "active issue-status-filter"
        )
        XCTAssertTrue(semanticText(activeStatus).contains("Filter status: In progress"), "active status filter should expose its selection")
        let activeClear = try require(app.buttons["issue-filters-clear"], "issue-filters-clear after status filter")
        XCTAssertGreaterThan(activeStatus.frame.width, 120, "active status filter should retain readable width")
        XCTAssertGreaterThan(activeStatus.frame.height, 20, "active status filter should retain visible height")
        XCTAssertTrue(hostWindow.frame.contains(activeStatus.frame), "active status filter should remain in the host window")
        XCTAssertTrue(hostWindow.frame.contains(activeClear.frame), "Clear filters should remain in the host window")
        XCTAssertFalse(activeStatus.frame.intersects(activeClear.frame), "status filter and Clear filters must not overlap")
        XCTAssertTrue(waitForSemanticText("3 of 5 Jira issues", in: issueListSummary), "In Progress should show three fixture issues")

        // Add a non-matching local query, then clear both controls with the explicit action.
        try setValue("no matching issue", identifier: "issue-search")
        XCTAssertFalse(app.descendants(matching: .any)["issue-search-submit"].exists, "non-key text should not offer Find key")
        XCTAssertTrue(waitForSemanticText("0 of 5 Jira issues", in: issueListSummary), "combined filters should expose zero results")
        let clearCombinedFilters = try require(app.buttons["issue-filters-clear"], "issue-filters-clear after combined filter")
        clearCombinedFilters.click()
        XCTAssertEqual(axValue(issueSearch), "", "Clear filters should reset the combined search input")
        XCTAssertTrue(waitForSemanticText("5 Jira issues", in: issueListSummary), "Clear filters should restore the full result count")
        let resetStatus = try require(
            app.descendants(matching: .any)["issue-status-filter"],
            "reset issue-status-filter"
        )
        XCTAssertTrue(semanticText(resetStatus).contains("Filter status: All statuses"), "Clear filters should restore All statuses")

        // A locally known, syntactically valid key is still an explicit lookup intent, but the
        // fixture must resolve it locally rather than manufacturing a remote error.
        issueSearch.click()
        try setValue("DESK-171", identifier: "issue-search")
        let searchSubmit = try require(app.buttons["issue-search-submit"], "issue-search-submit for valid Jira key")
        XCTAssertEqual(searchSubmit.label.isEmpty ? searchSubmit.title : searchSubmit.label, "Find key")
        XCTAssertTrue(hostWindow.frame.contains(searchSubmit.frame), "contextual key action should stay inside the host window")
        searchSubmit.click()
        let keyLookupDetail = try require(app.descendants(matching: .any)["issue-detail"], "issue-detail after local key lookup")
        let selectedDetailText = [keyLookupDetail.label, keyLookupDetail.title, keyLookupDetail.value as? String ?? ""].joined(separator: " ")
        XCTAssertTrue(selectedDetailText.contains("DESK-171"), "exact local key lookup should select DESK-171")
        XCTAssertTrue(app.descendants(matching: .any)["issues-table"].exists, "opening a key should keep the compact issue list beside detail")
        let finalClear = try require(app.buttons["issue-filters-clear"], "issue-filters-clear after key lookup")
        finalClear.click()
        let anotherIssueRow = try require(app.descendants(matching: .any)["issue-row-DESK-184"], "issue-row-DESK-184 beside open detail")
        anotherIssueRow.click()
        let selectedAnotherDetail = try require(app.descendants(matching: .any)["issue-detail"], "issue-detail after selecting another row")
        let anotherDetailExpectation = XCTNSPredicateExpectation(
            predicate: NSPredicate { object, _ in
                guard let detail = object as? XCUIElement else { return false }
                let identity = [detail.label, detail.title, detail.value as? String ?? ""].joined(separator: " ")
                return identity.contains("DESK-184")
            },
            object: selectedAnotherDetail
        )
        XCTAssertTrue(XCTWaiter.wait(for: [anotherDetailExpectation], timeout: 8) == .completed, "selecting another visible row should update the existing detail pane")
        XCTAssertTrue(app.descendants(matching: .any)["issues-table"].exists, "the issue table should remain visible while the selected detail changes")
        let detailBack = try require(app.buttons["issue-detail-back"], "issue-detail-back")
        XCTAssertTrue(semanticText(detailBack).contains("Back to issues"), "detail should expose a semantic overview return action")
        detailBack.click()
        let restoredTable = try require(app.descendants(matching: .any)["issues-table"], "issues-table after closing details")
        XCTAssertFalse(app.descendants(matching: .any)["issue-detail"].exists, "Back to issues should close the detail pane")
        XCTAssertGreaterThan(restoredTable.frame.width, initialOverviewTableWidth * 0.85, "closing details should restore the full-width overview")

        let sidebar = try require(
            app.descendants(matching: .any)["dashboard-sidebar"],
            "dashboard-sidebar"
        )
        let sidebarActions = try require(
            app.descendants(matching: .any)["sidebar-profile-actions"],
            "sidebar-profile-actions"
        )
        let profile = try require(
            app.descendants(matching: .any)["sidebar-profile"],
            "sidebar-profile"
        )
        let syncStatus = try require(
            app.descendants(matching: .any)["sidebar-sync-status"],
            "sidebar-sync-status"
        )
        let expectedRefreshCopy = "Updated · 5 issues · 3 new updates"
        let refreshCopyCandidates = [
            syncStatus.label,
            syncStatus.title,
            syncStatus.value as? String ?? "",
        ]
        XCTAssertTrue(
            refreshCopyCandidates.contains(expectedRefreshCopy),
            "fixture refresh status should expose the concise post-refresh copy"
        )
        for rejectedCopy in [
            "Refresh complete",
            "desktop notification",
            "accepted by desktop service",
            "local update",
        ] {
            XCTAssertFalse(
                refreshCopyCandidates.contains(where: { $0.localizedCaseInsensitiveContains(rejectedCopy) }),
                "refresh status must not regress to legacy copy: \(rejectedCopy)"
            )
        }

        let refreshNodes = app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier == %@", "sidebar-refresh")
        )
        XCTAssertEqual(refreshNodes.count, 1, "expanded sidebar should expose one refresh action")
        let refresh = try require(app.buttons["sidebar-refresh"], "sidebar-refresh")
        XCTAssertEqual(
            [refresh.label, refresh.title, refresh.value as? String ?? ""].first(where: { !$0.isEmpty }),
            "Refresh Jira",
            "sidebar refresh should retain its stable action label"
        )
        XCTAssertGreaterThan(refresh.frame.width, 0)
        XCTAssertLessThanOrEqual(refresh.frame.width, 24, "sidebar refresh should be icon-sized")
        XCTAssertGreaterThan(refresh.frame.height, 0)
        XCTAssertLessThanOrEqual(refresh.frame.height, 24, "sidebar refresh should be icon-sized")
        XCTAssertTrue(sidebarActions.frame.contains(refresh.frame), "refresh should remain in profile actions")
        XCTAssertTrue(sidebarActions.frame.contains(profile.frame), "profile should remain in profile actions")
        XCTAssertFalse(refresh.frame.intersects(profile.frame), "refresh and profile should not overlap")
        XCTAssertGreaterThan(refresh.frame.minX, profile.frame.maxX, "refresh should be to the right of profile")
        XCTAssertLessThanOrEqual(
            abs(refresh.frame.midY - profile.frame.midY),
            4,
            "refresh should align with the profile center line"
        )

        let sidebarToggle = try require(
            app.descendants(matching: .any)["sidebar-toggle"],
            "sidebar-toggle"
        )
        sidebarToggle.click()
        let collapsedRefreshNodes = app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier == %@", "sidebar-refresh")
        )
        XCTAssertEqual(collapsedRefreshNodes.count, 1, "collapsed sidebar should not duplicate refresh")
        let collapsedRefresh = try require(app.buttons["sidebar-refresh"], "sidebar-refresh after collapse")
        let collapsedProfile = try require(
            app.descendants(matching: .any)["sidebar-profile"],
            "sidebar-profile after collapse"
        )
        XCTAssertLessThanOrEqual(collapsedRefresh.frame.width, 24, "collapsed refresh should be icon-sized")
        XCTAssertLessThanOrEqual(collapsedRefresh.frame.height, 24, "collapsed refresh should be icon-sized")
        XCTAssertFalse(
            collapsedRefresh.frame.intersects(collapsedProfile.frame),
            "collapsed refresh and profile should not overlap"
        )
        XCTAssertTrue(sidebar.frame.contains(collapsedRefresh.frame), "collapsed refresh should stay in the sidebar")
        XCTAssertTrue(sidebar.frame.contains(collapsedProfile.frame), "collapsed profile should stay in the sidebar")
        XCTAssertGreaterThan(
            collapsedRefresh.frame.minY,
            collapsedProfile.frame.maxY,
            "collapsed refresh should be vertically below the profile"
        )

        for key in ["DESK-184", "DESK-179", "DESK-176", "DESK-171"] {
            let row = try require(
                app.descendants(matching: .any)["issue-row-\(key)"],
                "issue-row-\(key)"
            )
            let rowSemanticText = [row.label, row.title, row.value as? String ?? ""]
                .joined(separator: " ")
            XCTAssertTrue(
                rowSemanticText.contains(key),
                "\(key) table row should retain its issue identity"
            )
            XCTAssertGreaterThan(row.frame.width, 300, "\(key) row should have meaningful bounded width")
            XCTAssertGreaterThan(row.frame.height, 30, "\(key) row should remain readable")
            XCTAssertLessThan(row.frame.height, 70, "\(key) row should remain compact")
            XCTAssertLessThan(row.frame.width, 2_000, "\(key) row width should remain finite and bounded")
            XCTAssertLessThan(row.frame.height, 500, "\(key) row height should remain finite and bounded")
            XCTAssertTrue(
                row.frame.minX.isFinite
                    && row.frame.maxX.isFinite
                    && row.frame.minY.isFinite
                    && row.frame.maxY.isFinite,
                "\(key) row position should remain finite"
            )
        }
        let row = try require(app.descendants(matching: .any)["issue-row-DESK-179"], "issue-row-DESK-179")
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
        let compactTable = try require(app.descendants(matching: .any)["issues-table"], "compact issues table beside detail")
        XCTAssertLessThan(compactTable.frame.width, initialOverviewTableWidth * 0.7, "opening detail should give the detail pane its own region")
        XCTAssertGreaterThanOrEqual(compactTable.frame.width, 280, "table viewport should retain a usable width beside detail")
        XCTAssertTrue(hostWindow.frame.contains(compactTable.frame), "table viewport should stay inside the host window")
        XCTAssertFalse(detail.frame.intersects(compactTable.frame), "compact list and detail should occupy separate regions")
        XCTAssertLessThan(row.frame.height, 45, "selecting an issue should keep the table row compact")
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

        let updatedHeader = try require(
            app.descendants(matching: .any)["issues-column-updated"],
            "Updated column header in the horizontally scrollable table"
        )
        let updatedCell = try require(
            app.descendants(matching: .any)["issue-cell-updated-DESK-179"],
            "Updated cell in the horizontally scrollable table"
        )
        compactTable.scroll(byDeltaX: 600, deltaY: 0)
        XCTAssertTrue(compactTable.frame.contains(updatedHeader.frame), "horizontal scroll should bring the Updated header into the table viewport")
        XCTAssertTrue(compactTable.frame.contains(updatedCell.frame), "header and row content should scroll together into the table viewport")
        XCTAssertLessThanOrEqual(abs(updatedHeader.frame.minX - updatedCell.frame.minX), 12, "Updated cell should remain aligned with its header after horizontal scrolling")
        compactTable.scroll(byDeltaX: -600, deltaY: 0)

        let statusPill = try require(
            app.descendants(matching: .any)["issue-detail-status-pill"],
            "issue-detail-status-pill"
        )
        XCTAssertTrue(
            semanticText(statusPill).contains("Status: To Do"),
            "the header should expose the current status as static text"
        )
        XCTAssertFalse(
            app.descendants(matching: .any)["issue-detail-type"].exists,
            "collapsed Details should hide issue type metadata"
        )
        let priorityBadge = try require(
            app.descendants(matching: .any)["priority-badge-detail-DESK-179"],
            "priority-badge-detail-DESK-179"
        )
        let prioritySemanticText = [
            priorityBadge.label,
            priorityBadge.title,
            priorityBadge.value as? String ?? "",
        ]
        XCTAssertTrue(
            prioritySemanticText.contains("Priority: Highest"),
            "detail metadata should expose the exact priority semantically"
        )
        let metadataControls = [statusPill, priorityBadge]
        for (index, control) in metadataControls.enumerated() {
            XCTAssertGreaterThan(control.frame.width, 0, "metadata control \(index) should have visible width")
            XCTAssertGreaterThan(control.frame.height, 0, "metadata control \(index) should have visible height")
            XCTAssertLessThan(control.frame.width, 500, "metadata control \(index) should remain bounded")
            XCTAssertLessThan(control.frame.height, 100, "metadata control \(index) should remain bounded")
            XCTAssertTrue(detail.frame.contains(control.frame), "metadata control \(index) should be in issue detail")
        }
        let overflowTrigger = try require(
            app.buttons["issue-detail-overflow-trigger"],
            "issue-detail-overflow-trigger"
        )
        overflowTrigger.click()
        let assigneeAction = try require(
            app.buttons["change-assignee"],
            "change-assignee trigger in issue overflow menu"
        )
        XCTAssertEqual(assigneeAction.label.isEmpty ? assigneeAction.title : assigneeAction.label, "Change assignee")
        assertFiniteBounded(assigneeAction.frame, name: "change-assignee")
        overflowTrigger.click()
        let actionBar = try require(
            app.descendants(matching: .any)["issue-detail-action-bar"],
            "issue-detail-action-bar"
        )
        let addComment = try require(
            app.buttons["issue-detail-add-comment"],
            "issue-detail-add-comment"
        )
        let statusTrigger = try require(
            app.descendants(matching: .any)["issue-status-trigger"],
            "issue-status-trigger in action toolbar"
        )
        for (identifier, control) in [
            ("issue-detail-add-comment", addComment),
            ("issue-status-trigger", statusTrigger),
        ] {
            assertFiniteBounded(control.frame, name: identifier)
            XCTAssertTrue(actionBar.frame.contains(control.frame), "\(identifier) should stay inside the action bar")
        }
        let legacyActionsHeading = app.descendants(matching: .any).matching(
            NSPredicate(format: "label == %@ OR title == %@", "Jira issue actions", "Jira issue actions")
        )
        XCTAssertEqual(legacyActionsHeading.count, 0, "legacy Jira issue actions heading should be absent")

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

        let storyRow = try require(app.descendants(matching: .any)["issue-row-DESK-184"], "issue-row-DESK-184")
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

        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "issues-issue-types-final"
        screenshot.lifetime = .keepAlways
        add(screenshot)
    }

    func testRichContent() throws {
        try launchFixture(scenario: "rich-content")
        let richFixtureRow = try require(app.descendants(matching: .any)["issue-row-DESK-184"], "rich-content issue-row-DESK-184")
        richFixtureRow.click()
        let markdownSurface = try require(
            app.descendants(matching: .any)["issue-description-markdown"],
            "issue-description-markdown"
        )
        XCTAssertGreaterThan(markdownSurface.frame.width, 0, "Markdown description should have a visible width")
        XCTAssertGreaterThan(markdownSurface.frame.height, 80, "Markdown description should lay out multiple rendered lines")
        let description = try require(
            app.descendants(matching: .any)["issue-detail-description"],
            "issue-detail-description"
        )
        let descriptionSemanticText = [
            description.label,
            description.title,
            description.value as? String ?? "",
        ].joined(separator: " ")
        for expected in [
            "Parent",
            "Problem",
            "The API bug",
            "UploadedRecordingsProcessing::RecordingFilePreparer#s3_filename",
        ] {
            XCTAssertTrue(
                descriptionSemanticText.contains(expected),
                "Markdown description should expose rendered semantic text: \(expected)"
            )
        }
        for rejected in ["## Parent", "## Problem", "### The API bug", "`"] {
            XCTAssertFalse(
                descriptionSemanticText.contains(rejected),
                "Markdown description should not expose source delimiter: \(rejected)"
            )
        }
        let issueDetail = try require(
            app.descendants(matching: .any)["issue-detail"],
            "issue-detail"
        )
        let commentComposer = try require(
            app.descendants(matching: .any)["comment-composer"],
            "comment-composer"
        )
        let commentActions = try require(
            app.descendants(matching: .any)["comment-composer-actions"],
            "comment-composer-actions"
        )
        let postComment = try require(app.buttons["post-comment"], "post-comment")
        XCTAssertGreaterThan(postComment.frame.width, 0, "Post comment should have visible width")
        XCTAssertGreaterThan(postComment.frame.height, 0, "Post comment should have visible height")
        XCTAssertLessThan(
            postComment.frame.width,
            commentComposer.frame.width * 0.6,
            "Post comment should retain intrinsic compact width"
        )
        XCTAssertTrue(commentComposer.frame.contains(commentActions.frame), "comment actions should remain inside composer")
        XCTAssertTrue(commentActions.frame.contains(postComment.frame), "Post comment should remain inside its action row")
        XCTAssertTrue(
            commentComposer.frame.width.isFinite && commentComposer.frame.height.isFinite,
            "comment composer frame should remain finite even when below the rich-content viewport"
        )
        let richLinkNodes = app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "rich-text-link-")
        )
        XCTAssertEqual(richLinkNodes.count, 2, "rich fixture should expose only its two safe link identifiers")
        let richLinks = app.links.matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "rich-text-link-")
        )
        XCTAssertEqual(richLinks.count, 2, "rich fixture should expose two semantic Link roles")
        let expectedLinkLabels: Set<String> = [
            "Open link: fixture documentation",
            "Open Jira issue ENG-43",
        ]
        var observedLinkLabels = [String]()
        for index in 0..<richLinks.count {
            let link = try require(
                richLinks.element(boundBy: index),
                "rich link \(index)"
            )
            let semanticCandidates = [
                link.label,
                link.title,
                link.value as? String ?? "",
            ].filter { !$0.isEmpty }
            let matchingLabels = semanticCandidates.filter { expectedLinkLabels.contains($0) }
            XCTAssertEqual(matchingLabels.count, 1, "rich link \(index) should expose one expected semantic label")
            if let matchingLabel = matchingLabels.first {
                observedLinkLabels.append(matchingLabel)
            }
            XCTAssertEqual(link.elementType, .link, "safe rich content should expose a Link role")
            XCTAssertGreaterThan(link.frame.width, 20, "safe link should have meaningful width")
            XCTAssertGreaterThan(link.frame.height, 10, "safe link should have meaningful height")
            XCTAssertLessThan(link.frame.width, 600, "safe link width should remain bounded")
            XCTAssertLessThan(link.frame.height, 100, "safe link height should remain bounded")
            XCTAssertTrue(issueDetail.frame.contains(link.frame), "safe link should remain inside issue detail")
        }
        XCTAssertEqual(Set(observedLinkLabels), expectedLinkLabels, "safe rich links should expose the expected labels")
        let hostileLinks = richLinkNodes.matching(
            NSPredicate(format: "identifier BEGINSWITH %@ AND label CONTAINS %@", "rich-text-link-", "hostile scheme")
        )
        XCTAssertEqual(hostileLinks.count, 0, "hostile schemes must not expose a clickable Link role")
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
                NSPredicate(format: "identifier BEGINSWITH %@", "rich-text-expand-")
            ).firstMatch,
            "Details expand"
        )
        let nestedExpand = try require(
            app.descendants(matching: .any).matching(
                NSPredicate(format: "identifier BEGINSWITH %@", "rich-text-nested-expand-")
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
            let paragraphCandidate = app.descendants(matching: .any).matching(
                NSPredicate(format: "identifier BEGINSWITH %@ AND title == %@", "rich-text-paragraph-", expected)
            ).firstMatch
            let paragraph = try require(
                paragraphCandidate,
                "rich paragraph with title \(expected)"
            )
            let semanticTitle = paragraph.title.isEmpty ? paragraph.label : paragraph.title
            XCTAssertEqual(
                semanticTitle,
                expected,
                "rich paragraph should expose exact semantic title"
            )
        }

        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "rich-content-final"
        screenshot.lifetime = .keepAlways
        add(screenshot)
    }

    func testRelationships() throws {
        try launchFixture(scenario: "issues")

        let row = try require(app.descendants(matching: .any)["issue-row-DESK-184"], "issue-row-DESK-184")
        row.click()
        let initialDetail = try require(app.descendants(matching: .any)["issue-detail"], "issue-detail for DESK-184")
        XCTAssertTrue(waitForSemanticText("Issue detail for DESK-184", in: initialDetail), "DESK-184 detail should be selected")

        let breadcrumbs = try require(
            app.descendants(matching: .any)["issue-detail-breadcrumbs"],
            "issue-detail-breadcrumbs"
        )
        XCTAssertGreaterThan(breadcrumbs.frame.width, 0, "issue breadcrumbs should have visible width")
        XCTAssertGreaterThan(breadcrumbs.frame.height, 0, "issue breadcrumbs should have visible height")
        let detailsTrigger = try require(
            app.buttons["issue-detail-details-trigger"],
            "issue-detail-details-trigger"
        )
        XCTAssertFalse(
            app.descendants(matching: .any)["issue-detail-parent"].exists,
            "collapsed Details should hide parent metadata"
        )
        detailsTrigger.click()
        let parentMetadata = try require(
            app.descendants(matching: .any)["issue-detail-parent"],
            "DESK-184 parent metadata inside expanded Details"
        )
        XCTAssertTrue(semanticText(parentMetadata).contains("DESK-171"), "DESK-184 Details should expose its parent identity")
        detailsTrigger.click()
        let overflow = try require(
            app.buttons["issue-detail-overflow-trigger"],
            "issue-detail-overflow-trigger"
        )
        overflow.click()
        let openParent = try require(
            app.buttons["issue-detail-overflow-open-parent"],
            "issue-detail-overflow-open-parent"
        )
        XCTAssertTrue(semanticText(openParent).contains("Open parent issue"))
        openParent.click()
        XCTAssertTrue(waitForSemanticText("Issue detail for DESK-171", in: initialDetail), "parent navigation should select DESK-171")
        let parentIssueDetails = try require(
            app.descendants(matching: .any)["issue-detail-details"],
            "parent issue Details"
        )
        XCTAssertTrue(semanticText(parentIssueDetails).contains("Details"))
        let parentDetailsTrigger = try require(
            app.buttons["issue-detail-details-trigger"],
            "parent issue Details trigger"
        )
        parentDetailsTrigger.click()
        let parentMetadataEmpty = try require(
            app.descendants(matching: .any)["issue-detail-parent"],
            "parent issue empty parent metadata"
        )
        XCTAssertTrue(
            semanticText(parentMetadataEmpty).contains("None"),
            "the parent fixture should expose its empty parent state"
        )
        XCTAssertFalse(
            app.descendants(matching: .any)["issue-detail-linked-issues"].exists,
            "the parent fixture should expose the empty linked-issues state"
        )

        row.click()
        let detail = try require(app.descendants(matching: .any)["issue-detail"], "issue-detail with relationships")
        let linked = try require(
            app.descendants(matching: .any)["issue-detail-linked-issues"],
            "issue-detail-linked-issues"
        )
        XCTAssertGreaterThan(linked.frame.width, 180, "linked issue surface should retain usable width")
        XCTAssertLessThan(linked.frame.height, 500, "linked issue surface should remain bounded")
        let expectedLinks = [(0, "blocks", "DESK-179"), (1, "is blocked by", "DESK-176")]
        for (index, direction, key) in expectedLinks {
            let linkRow = try require(
                app.descendants(matching: .any)["issue-detail-linked-\(index)"],
                "issue-detail-linked-\(index)"
            )
            assertFiniteBounded(linkRow.frame, name: "issue-detail-linked-\(index)")
            let link = try require(
                app.buttons["issue-detail-linked-key-\(index)"],
                "issue-detail-linked-key-\(index)"
            )
            XCTAssertTrue(semanticText(linkRow).contains(direction), "linked issue \(index) should expose its direction")
            XCTAssertTrue(semanticText(linkRow).contains(key), "linked issue \(index) should expose its key")
            XCTAssertTrue(semanticText(link).contains(key), "linked issue key \(index) should remain actionable")
        }

        let firstLink = try require(app.buttons["issue-detail-linked-key-0"], "first linked issue key")
        XCTAssertGreaterThanOrEqual(linked.frame.minX, detail.frame.minX - 1, "linked issue surface should stay inside detail horizontally")
        XCTAssertLessThanOrEqual(linked.frame.maxX, detail.frame.maxX + 1, "linked issue surface should stay inside detail horizontally")
        for index in 0..<2 {
            let row = app.descendants(matching: .any)["issue-detail-linked-\(index)"]
            XCTAssertGreaterThanOrEqual(row.frame.minX, detail.frame.minX - 1, "linked row \(index) should stay inside detail horizontally")
            XCTAssertLessThanOrEqual(row.frame.maxX, detail.frame.maxX + 1, "linked row \(index) should stay inside detail horizontally")
        }
        for _ in 0..<6 where !firstLink.isHittable {
            detail.swipeUp()
        }
        XCTAssertTrue(firstLink.isHittable, "first linked issue key should become hittable within a bounded scroll")
        firstLink.click()
        XCTAssertTrue(waitForSemanticText("Issue detail for DESK-179", in: detail), "first linked navigation should select DESK-179")

        row.click()
        let secondLink = try require(app.buttons["issue-detail-linked-key-1"], "second linked issue key")
        for _ in 0..<6 where !secondLink.isHittable {
            detail.swipeUp()
        }
        XCTAssertTrue(secondLink.isHittable, "second linked issue key should become hittable within a bounded scroll")
        secondLink.click()
        XCTAssertTrue(waitForSemanticText("Issue detail for DESK-176", in: detail), "second linked navigation should select DESK-176")
    }

    func testKitComponentSemanticsAndBoundedGeometry() throws {
        try launchFixture(scenario: "components")
        let componentFixtureRow = try require(app.descendants(matching: .any)["issue-row-DESK-184"], "components issue-row-DESK-184")
        componentFixtureRow.click()

        let detail = try require(
            app.descendants(matching: .any)["issue-detail"],
            "issue-detail"
        )

        // The status surface and Jira labels are display-only metadata tags. Keep this semantic
        // check independent of their painted color and shape.
        for (identifier, expectedText) in [
            ("issue-detail-label-notifications", "Label notifications"),
            ("issue-detail-label-phase-1", "Label phase-1"),
        ] {
            var tag = app.descendants(matching: .any)[identifier]
            for _ in 0..<8 where !tag.isHittable {
                detail.swipeUp()
                tag = app.descendants(matching: .any)[identifier]
            }
            tag = try require(tag, identifier)
            XCTAssertTrue(semanticText(tag).contains(expectedText), "\(identifier) should expose tag semantics")
            XCTAssertTrue(detail.frame.contains(tag.frame), "\(identifier) should remain inside issue detail")
            assertFiniteBounded(tag.frame, name: identifier)
            XCTAssertLessThan(tag.frame.width, 500, "\(identifier) should remain compact")
            XCTAssertLessThan(tag.frame.height, 100, "\(identifier) should remain compact")
        }

        // The detail fixture has one authenticated-download attachment. The attachment row and
        // action must remain discoverable through the kit presentation component without firing
        // the write/download path.
        var attachmentCandidate = app.buttons["download-attachment-comment-fixture-image"]
        for _ in 0..<8 where !attachmentCandidate.isHittable {
            detail.swipeUp()
            attachmentCandidate = app.buttons["download-attachment-comment-fixture-image"]
        }
        let attachmentAction = try require(attachmentCandidate, "download-attachment-comment-fixture-image")
        XCTAssertTrue(semanticText(attachmentAction).contains("Download"), "attachment action should retain download semantics")
        assertFiniteBounded(attachmentAction.frame, name: "download-attachment-comment-fixture-image")
        XCTAssertTrue(detail.frame.contains(attachmentAction.frame), "attachment action should remain inside issue detail")

        // DESK-179 is deliberately detail-loaded with no description. Relaunch the isolated
        // fixture so this check does not depend on a network lookup or a prior interaction.
        app.terminate()
        try launchFixture(scenario: "issues")
        let issueRow = try require(app.descendants(matching: .any)["issue-row-DESK-179"], "issue-row-DESK-179")
        issueRow.click()
        let empty = try require(
            app.descendants(matching: .any)["issue-detail-description"],
            "issue-detail-description"
        )
        XCTAssertTrue(semanticText(empty).contains("No description supplied."), "empty state should expose its copy")
        assertFiniteBounded(empty.frame, name: "issue-detail-description")
        XCTAssertFalse(
            app.descendants(matching: .any).matching(NSPredicate(format: "label == %@", "Loading issue details…")).firstMatch.exists,
            "empty detail should not regress to a loading state"
        )
        let search = try require(app.descendants(matching: .any)["issue-search"], "issue-search")
        search.click()
        try setValue("no matching fixture", identifier: "issue-search")
        let filteredEmpty = try require(app.descendants(matching: .any)["issues-empty"], "issues-empty")
        XCTAssertTrue(
            semanticText(filteredEmpty).contains("No issues match the current search and status filters"),
            "Empty component should expose its explicit status label"
        )
        assertFiniteBounded(filteredEmpty.frame, name: "issues-empty")
    }

    func testAlertComponentSemantics() throws {
        try launchFixture(scenario: "alert")
        let alertFixtureRow = try require(app.descendants(matching: .any)["issue-row-DESK-184"], "alert issue-row-DESK-184")
        alertFixtureRow.click()
        let alert = app.descendants(matching: .any)["issue-detail-error"]
        let fallback = app.descendants(matching: .any)["issue-detail-error-surface"]
        let surface = alert.exists ? alert : fallback
        _ = try require(surface, "issue-detail-error")
        XCTAssertTrue(semanticText(surface).contains("Unable to load issue details"), "Alert should expose its title")
        XCTAssertTrue(semanticText(surface).contains("Jira is unreachable"), "Alert should expose recovery context")
        assertFiniteBounded(surface.frame, name: "issue-detail-error")
        let hostWindow = try require(app.windows.firstMatch, "fixture host window")
        XCTAssertTrue(hostWindow.frame.contains(surface.frame), "Alert should remain inside the host window")
    }

    func testAssigneeListConfirmation() throws {
        try launchFixture(scenario: "assignee")
        let assigneeFixtureRow = try require(app.descendants(matching: .any)["issue-row-DESK-184"], "assignee issue-row-DESK-184")
        assigneeFixtureRow.click()
        let list = try require(app.descendants(matching: .any)["assignee-list"], "assignee-list")
        let unassigned = try require(app.descendants(matching: .any)["assignee-0"], "assignee-0")
        let firstUser = try require(app.descendants(matching: .any)["assignee-1"], "assignee-1")
        XCTAssertTrue(semanticText(unassigned).contains("Unassigned"), "assignee List should expose Unassigned")
        XCTAssertTrue(semanticText(firstUser).contains("Amina Yusuf"), "assignee List should expose fixture users")
        assertFiniteBounded(list.frame, name: "assignee-list")
        app.typeKey(XCUIKeyboardKey.downArrow.rawValue, modifierFlags: [])
        app.typeKey(XCUIKeyboardKey.downArrow.rawValue, modifierFlags: [])
        app.typeKey(XCUIKeyboardKey.return.rawValue, modifierFlags: [])
        let confirmation = try require(
            app.descendants(matching: .any)["assignee-confirmation"],
            "assignee-confirmation"
        )
        XCTAssertTrue(semanticText(confirmation).contains("Assign DESK-184 to Amina Yusuf?"), "confirmation should expose the full assignment target")
        XCTAssertTrue(semanticText(confirmation).contains("Amina Yusuf"), "assignee selection should enter confirmation state")
        XCTAssertTrue(app.buttons["confirm-assignee"].exists, "confirmation action should be discoverable")
    }

    func testVirtualizedIssueRowsRemainIdentifiableAfterScrollAndFilter() throws {
        try launchFixture(scenario: "virtualized")
        let list = try require(app.descendants(matching: .any)["issues-table"], "issues-table")
        let summary = try require(app.descendants(matching: .any)["issue-list-summary"], "issue-list-summary")
        XCTAssertTrue(waitForSemanticText("45 Jira issues", in: summary), "virtualized fixture should expose its full count")

        let first = try require(app.descendants(matching: .any)["issue-row-DESK-184"], "issue-row-DESK-184")
        assertFiniteBounded(first.frame, name: "issue-row-DESK-184")
        XCTAssertTrue(list.frame.intersects(first.frame), "first virtualized row should be in or at the list viewport")

        var later = app.descendants(matching: .any)["issue-row-VIRT-39"]
        for _ in 0..<16 where !later.exists {
            list.swipeUp()
            later = app.descendants(matching: .any)["issue-row-VIRT-39"]
        }
        let realizedLater = try require(later, "issue-row-VIRT-39")
        XCTAssertTrue(list.frame.intersects(realizedLater.frame), "scrolled virtualized row should intersect the list viewport")
        XCTAssertTrue(semanticText(realizedLater).contains("VIRT-39"), "scrolled row should retain its stable issue identity")
        assertFiniteBounded(realizedLater.frame, name: "issue-row-VIRT-39")

        let search = try require(app.descendants(matching: .any)["issue-search"], "issue-search")
        search.click()
        try setValue("VIRT-39", identifier: "issue-search")
        let filtered = try require(app.descendants(matching: .any)["issue-row-VIRT-39"], "filtered issue-row-VIRT-39")
        XCTAssertTrue(waitForSemanticText("1 of 45 Jira issues", in: summary), "filter should retain the virtualized total")
        XCTAssertTrue(semanticText(filtered).contains("VIRT-39"), "filtered result should retain stable identity")
        assertFiniteBounded(filtered.frame, name: "filtered issue-row-VIRT-39")
    }

    func testVirtualizedUpdateExpansionRetainsGeometry() throws {
        try launchFixture(scenario: "virtualized")
        let nav = try require(app.descendants(matching: .any)["nav-updates"], "nav-updates")
        nav.click()
        let list = try require(app.descendants(matching: .any)["update-list"], "update-list")
        let card = try require(app.descendants(matching: .any)["update-card-0"], "update-card-0")
        let expand = try require(app.buttons["Show 9 more"], "Show 9 more")
        let collapsedHeight = card.frame.height
        expand.click()
        let collapse = try require(app.buttons["Show less"], "Show less")
        XCTAssertGreaterThan(card.frame.height, collapsedHeight, "expanded update group should grow its card")
        XCTAssertLessThan(list.frame.height, 1_000, "virtualized update viewport should remain bounded")
        assertFiniteBounded(collapse.frame, name: "Show less")
        collapse.click()
        XCTAssertLessThanOrEqual(card.frame.height, collapsedHeight + 4, "collapsing update group should restore its card height")
    }

    func testVirtualizedCoverage() throws {
        try testVirtualizedIssueRowsRemainIdentifiableAfterScrollAndFilter()
        try testVirtualizedUpdateExpansionRetainsGeometry()
    }

    func testCommentConfirmationActions() throws {
        try launchFixture(scenario: "comment-confirmation")

        let composer = try require(
            app.descendants(matching: .any)["comment-composer"],
            "comment-composer"
        )
        let hostWindow = try require(app.windows.firstMatch, "fixture host window")
        let actions = try require(
            app.descendants(matching: .any)["comment-composer-actions"],
            "comment-composer-actions"
        )
        let postNow = try require(app.buttons["post-comment-now"], "post-comment-now")
        let cancel = try require(app.buttons["cancel-comment"], "cancel-comment")
        let detail = try require(app.descendants(matching: .any)["issue-detail"], "issue-detail")

        for _ in 0..<8 where !postNow.isHittable || !cancel.isHittable {
            detail.swipeUp()
        }
        XCTAssertTrue(postNow.isHittable, "Post now should become hittable after scrolling issue detail")
        XCTAssertTrue(cancel.isHittable, "Cancel should become hittable after scrolling issue detail")

        XCTAssertGreaterThan(postNow.frame.width, 0, "Post now should have visible width")
        XCTAssertGreaterThan(cancel.frame.width, 0, "Cancel should have visible width")
        XCTAssertTrue(hostWindow.frame.contains(postNow.frame), "Post now should be visible inside the host window")
        XCTAssertTrue(hostWindow.frame.contains(cancel.frame), "Cancel should be visible inside the host window")
        XCTAssertLessThan(
            postNow.frame.width,
            actions.frame.width * 0.6,
            "Post now should retain intrinsic width in the confirmation action surface"
        )
        XCTAssertLessThan(
            cancel.frame.width,
            actions.frame.width * 0.6,
            "Cancel should retain intrinsic width in the confirmation action surface"
        )
        XCTAssertTrue(composer.frame.contains(actions.frame), "confirmation actions should remain inside composer")
        XCTAssertTrue(actions.frame.contains(postNow.frame), "Post now should remain inside action surface")
        XCTAssertTrue(actions.frame.contains(cancel.frame), "Cancel should remain inside action surface")
        XCTAssertLessThanOrEqual(
            abs(cancel.frame.maxX - actions.frame.maxX),
            2,
            "confirmation action group should be trailing-aligned in the narrow action surface"
        )
        XCTAssertLessThanOrEqual(
            abs(postNow.frame.midY - cancel.frame.midY),
            2,
            "confirmation actions should share a row at narrow width"
        )
        XCTAssertLessThanOrEqual(
            postNow.frame.maxX,
            cancel.frame.minX,
            "horizontal confirmation actions should not overlap"
        )
        XCTAssertEqual(postNow.label.isEmpty ? postNow.title : postNow.label, "Post now")
        XCTAssertEqual(cancel.label.isEmpty ? cancel.title : cancel.label, "Cancel")

        // This fixture is intentionally confirmation-only. The write action is never activated.
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "comment-confirmation-actions-final"
        screenshot.lifetime = .keepAlways
        add(screenshot)
    }

    func testSettings() throws {
        try launchFixture(scenario: "settings")
        let sidebar = try require(app.descendants(matching: .any)["dashboard-sidebar"], "dashboard-sidebar")
        let navIssues = try require(app.descendants(matching: .any)["nav-issues"], "nav-issues")
        let navSettings = try require(app.descendants(matching: .any)["nav-settings"], "nav-settings")
        navIssues.click()
        _ = try require(app.descendants(matching: .any)["issue-search"], "issue-search after selecting Issues")
        navSettings.click()
        XCTAssertTrue(waitForAbsence(app.descendants(matching: .any)["issue-search"]), "Settings should replace Issues content")
        _ = try require(app.descendants(matching: .any)["appearance-dark"], "appearance-dark")

        let settingsRoot = try require(app.descendants(matching: .any)["settings-root"], "settings-root")
        let categoryNavigation = try require(
            app.descendants(matching: .any)["settings-category-navigation"],
            "settings-category-navigation"
        )
        XCTAssertTrue(settingsRoot.frame.contains(categoryNavigation.frame), "category navigation should stay inside Settings")
        assertFiniteBounded(categoryNavigation.frame, name: "settings category navigation")
        let categories = [
            ("settings-category-appearance", "settings-content-appearance"),
            ("settings-category-issue-scope", "settings-content-issue-scope"),
            ("settings-category-team-tracker", "settings-content-team-tracker"),
            ("settings-category-desktop-notifications", "settings-content-desktop-notifications"),
            ("settings-category-saved-jira-login", "settings-content-saved-jira-login"),
        ]
        for (identifier, contentIdentifier) in categories {
            let category = try require(app.descendants(matching: .tab)[identifier], identifier)
            XCTAssertTrue(categoryNavigation.frame.contains(category.frame), "\(identifier) should stay inside category navigation")
            assertFiniteBounded(category.frame, name: identifier)
            category.click()
            let content = try require(app.descendants(matching: .any)[contentIdentifier], contentIdentifier)
            let selectedCategory = try require(app.descendants(matching: .tab)[identifier], "selected \(identifier)")
            let differentIdentifier = categories.first { $0.0 != identifier }!.0
            let differentCategory = try require(app.descendants(matching: .tab)[differentIdentifier], "inactive \(differentIdentifier)")
            XCTAssertTrue(
                waitForAXValue(["true", "1"], in: selectedCategory),
                "\(identifier) should expose accessibility value true/1 after activation"
            )
            XCTAssertTrue(
                waitForAXValue(["false", "0"], in: differentCategory),
                "\(differentIdentifier) should expose accessibility value false/0 when \(identifier) is active"
            )
            XCTAssertTrue(settingsRoot.frame.contains(content.frame), "\(contentIdentifier) should stay inside Settings")
            assertFiniteBounded(content.frame, name: contentIdentifier)
        }

        let profile = try require(app.buttons["sidebar-profile"], "sidebar-profile")
        let profileName = profile.label.isEmpty ? profile.title : profile.label
        XCTAssertFalse(profileName.isEmpty, "profile trigger should expose the account name")
        XCTAssertGreaterThan(profile.frame.width, 0)
        XCTAssertGreaterThan(profile.frame.height, 0)
        XCTAssertTrue(sidebar.frame.contains(profile.frame), "profile trigger should stay inside the sidebar")

        func openAccountMenu() throws {
            profile.click()
            for label in [
                "Appearance",
                "Issue scope",
                "Team tracker",
                "Desktop notifications",
                "Saved Jira login",
            ] {
                _ = try require(app.menuItems[label], "account menu item \(label)")
            }
        }

        func activateAccountMenuItem(_ label: String) throws {
            try openAccountMenu()
            let item = try require(app.menuItems[label], "account menu item \(label)")
            let frame = item.frame
            XCTAssertGreaterThan(frame.width, 0, "account menu item \(label) should have visible width")
            XCTAssertGreaterThan(frame.height, 0, "account menu item \(label) should have visible height")
            XCTAssertTrue(frame.minX.isFinite && frame.maxX.isFinite && frame.minY.isFinite && frame.maxY.isFinite,
                          "account menu item \(label) should have bounded geometry")
            item.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).click()
        }

        try activateAccountMenuItem("Appearance")
        _ = try require(app.descendants(matching: .any)["appearance-dark"], "appearance-dark")

        try activateAccountMenuItem("Issue scope")
        _ = try require(app.descendants(matching: .any)["JQL scope"], "JQL scope")
        _ = try require(app.buttons["Save and refresh"], "Save and refresh")

        try activateAccountMenuItem("Team tracker")
        _ = try require(app.descendants(matching: .any)["Team tracker members"], "Team tracker members")
        _ = try require(app.buttons["Save team"], "Save team")

        try activateAccountMenuItem("Desktop notifications")
        _ = try require(app.buttons["Send test notification"], "Send test notification")

        try activateAccountMenuItem("Saved Jira login")
        _ = try require(app.buttons["Forget saved Jira login"], "Forget saved Jira login")

        try activateAccountMenuItem("Appearance")
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
        XCTAssertLessThan(sidebar.frame.width, 100, "collapsed sidebar should use the icon-only rail")
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
        let collapsedIssues = try require(
            app.descendants(matching: .any)["nav-issues"],
            "nav-issues in collapsed sidebar"
        )
        let collapsedSettings = try require(
            app.descendants(matching: .any)["nav-settings"],
            "nav-settings in collapsed sidebar"
        )
        XCTAssertTrue(sidebar.frame.contains(collapsedSettings.frame), "Settings should remain inside the collapsed rail")
        assertFiniteBounded(collapsedSettings.frame, name: "collapsed nav-settings")
        collapsedIssues.click()
        _ = try require(app.descendants(matching: .any)["issue-search"], "issue-search from collapsed navigation")
        collapsedSettings.click()
        XCTAssertTrue(waitForAbsence(app.descendants(matching: .any)["issue-search"]), "collapsed Settings should replace Issues content")
        _ = try require(app.descendants(matching: .any)["appearance-dark"], "appearance-dark after collapsed navigation")
    }

    func testUpdates() throws {
        try launchFixture(scenario: "updates")
        let updates = try require(app.descendants(matching: .any)["nav-updates"], "nav-updates")
        updates.click()
        let hostWindow = try require(app.windows.firstMatch, "fixture host window")
        assertFiniteBounded(hostWindow.frame, name: "updates workspace")
        let updateList = try require(app.descendants(matching: .any)["update-list"], "update-list")
        let firstReadingPane = try require(
            app.groups["update-reading-pane"],
            "initial update-reading-pane for DESK-184"
        )
        assertFiniteBounded(updateList.frame, name: "update-list")
        assertFiniteBounded(firstReadingPane.frame, name: "initial DESK-184 update-reading-pane")
        let activityHeading = try require(
            app.descendants(matching: .any)["inbox-heading"],
            "Activity heading"
        )
        XCTAssertTrue(semanticText(activityHeading).contains("Activity"), "inbox should use the Activity heading")
        let inboxSearch = try require(
            app.descendants(matching: .any)["inbox-search"],
            "inbox-search"
        )
        let inboxStatus = try require(
            app.descendants(matching: .any)["inbox-status-trigger"],
            "inbox-status-trigger"
        )
        assertFiniteBounded(inboxSearch.frame, name: "inbox-search")
        assertFiniteBounded(inboxStatus.frame, name: "inbox-status-trigger")
        XCTAssertGreaterThan(inboxSearch.frame.width, 220, "inbox search should fit a useful local query")
        XCTAssertGreaterThan(inboxSearch.frame.height, 20, "inbox search should have a visible control height")
        XCTAssertGreaterThan(inboxStatus.frame.width, 120, "inbox status filter should have readable width")
        XCTAssertTrue(hostWindow.frame.contains(inboxSearch.frame), "inbox search should stay inside the host window")
        XCTAssertTrue(hostWindow.frame.contains(inboxStatus.frame), "inbox status filter should stay inside the host window")
        XCTAssertFalse(inboxSearch.frame.intersects(inboxStatus.frame), "inbox search and status filter must not overlap")
        XCTAssertTrue(
            semanticText(inboxStatus).contains("All statuses"),
            "inbox should start with all statuses visible"
        )
        let allTickets = try require(
            app.descendants(matching: .any)["updates-filter-all"],
            "updates-filter-all"
        )
        let unreadTickets = try require(
            app.descendants(matching: .any)["updates-filter-unread"],
            "updates-filter-unread"
        )
        assertFiniteBounded(allTickets.frame, name: "all activity filter")
        assertFiniteBounded(unreadTickets.frame, name: "unread activity filter")
        XCTAssertFalse(allTickets.frame.intersects(unreadTickets.frame), "activity scope filters must not overlap")
        XCTAssertTrue(
            semanticText(firstReadingPane).contains("DESK-184 local updates"),
            "the initial reading pane should expose its DESK-184 local update identity"
        )
        XCTAssertTrue(hostWindow.frame.contains(updateList.frame), "update list should remain inside the workspace")
        XCTAssertTrue(hostWindow.frame.contains(firstReadingPane.frame), "initial reading pane should remain inside the workspace")
        XCTAssertLessThanOrEqual(
            updateList.frame.maxX,
            firstReadingPane.frame.minX + 2,
            "Focus inbox list and initial reading pane should not overlap"
        )

        // Inbox search is local: matching the DESK-184 summary keeps that ticket and hides
        // other groups without issuing a Jira lookup.
        try setValue("Surface Jira", identifier: "inbox-search")
        let matchingGroup = try require(
            app.descendants(matching: .any)["update-card-0"],
            "DESK-184 group after local inbox search"
        )
        XCTAssertTrue(semanticText(matchingGroup).contains("DESK-184"), "local inbox search should retain the matching group")
        XCTAssertTrue(
            waitForAbsence(app.descendants(matching: .any)["update-card-1"]),
            "local inbox search should hide nonmatching groups"
        )
        XCTAssertFalse(
            app.descendants(matching: .any)["remote-lookup-error"].exists,
            "local inbox search should not trigger a Jira lookup"
        )
        inboxSearch.click()
        app.typeKey("a", modifierFlags: .command)
        app.typeKey(XCUIKeyboardKey.delete.rawValue, modifierFlags: [])
        XCTAssertEqual(axValue(inboxSearch), "", "clearing the inbox query should restore the full group list")
        _ = try require(app.descendants(matching: .any)["update-card-1"], "DESK-179 group after clearing inbox search")

        // Status selection also filters the local groups. DESK-184 is In Progress while
        // DESK-179 is To Do in the deterministic updates fixture.
        inboxStatus.click()
        _ = try require(app.menuItems["All statuses"], "inbox All statuses option")
        _ = try require(app.menuItems["To do"], "inbox To do option")
        let inProgressOption = try require(app.menuItems["In progress"], "inbox In Progress option")
        // Hover may preselect a row when the popup opens. Follow semantic aria-selected state
        // until the intended option is highlighted, then commit it from the keyboard.
        for _ in 0..<5 where !inProgressOption.isSelected {
            app.typeKey(XCUIKeyboardKey.downArrow.rawValue, modifierFlags: [])
        }
        XCTAssertTrue(inProgressOption.isSelected, "keyboard traversal should select In progress")
        app.typeKey(XCUIKeyboardKey.return.rawValue, modifierFlags: [])
        XCTAssertTrue(
            semanticText(inboxStatus).contains("In progress"),
            "inbox status filter should expose its selected status"
        )
        _ = try require(app.descendants(matching: .any)["update-card-0"], "DESK-184 group under In Progress filter")
        XCTAssertTrue(
            waitForAbsence(app.descendants(matching: .any)["update-card-1"]),
            "In Progress should hide the To Do DESK-179 group"
        )
        inboxStatus.click()
        let allStatusesOption = try require(app.menuItems["All statuses"], "inbox All statuses option")
        for _ in 0..<5 where !allStatusesOption.isSelected {
            app.typeKey(XCUIKeyboardKey.downArrow.rawValue, modifierFlags: [])
        }
        XCTAssertTrue(allStatusesOption.isSelected, "keyboard traversal should select All statuses")
        app.typeKey(XCUIKeyboardKey.return.rawValue, modifierFlags: [])
        XCTAssertTrue(semanticText(inboxStatus).contains("All statuses"), "deselecting In Progress should restore all statuses")

        let firstCard = try require(app.descendants(matching: .any)["update-card-0"], "update-card-0")
        let openFirst = try require(app.buttons["update-open-0"], "update-open-0")
        let firstCardFrameBeforeClick = firstCard.frame
        XCTAssertTrue(updateList.frame.contains(firstCard.frame), "first update card should stay inside the update list")
        XCTAssertTrue(firstCard.frame.contains(openFirst.frame), "first update action should stay inside its card")
        assertFiniteBounded(firstCard.frame, name: "update-card-0")
        assertFiniteBounded(openFirst.frame, name: "update-open-0")
        let secondCard = try require(app.descendants(matching: .any)["update-card-1"], "update-card-1")
        let openSecond = try require(app.buttons["update-open-1"], "update-open-1")
        let secondCardFrameBeforeClick = secondCard.frame
        XCTAssertTrue(updateList.frame.contains(secondCard.frame), "second update card should stay inside the update list")
        openFirst.click()

        let selectedFirstCard = try require(
            app.descendants(matching: .any)["update-card-0"],
            "initially selected update-card-0"
        )
        let unselectedSecondCard = try require(
            app.descendants(matching: .any)["update-card-1"],
            "unselected update-card-1"
        )
        let selectedCardScreenshot = hostWindow.screenshot()
        let railY = selectedFirstCard.frame.midY
        let railInner = try requireScreenshotColor(
            selectedCardScreenshot,
            at: CGPoint(x: selectedFirstCard.frame.minX + 0.5, y: railY),
            in: hostWindow,
            name: "selected update rail"
        )
        let railOuter = try requireScreenshotColor(
            selectedCardScreenshot,
            at: CGPoint(x: selectedFirstCard.frame.minX + 1.5, y: railY),
            in: hostWindow,
            name: "selected update rail inner edge"
        )
        let afterRail = try requireScreenshotColor(
            selectedCardScreenshot,
            at: CGPoint(x: selectedFirstCard.frame.minX + 2.5, y: railY),
            in: hostWindow,
            name: "selected update surface beside rail"
        )
        XCTAssertLessThan(rgbDistance(railInner, railOuter), 24,
                          "the selected rail should have consistent color across its 2 px width")
        XCTAssertGreaterThan(rgbDistance(railOuter, afterRail), 36,
                             "the selected rail should end after roughly 2 px")
        let selectedDivider = try requireScreenshotColor(
            selectedCardScreenshot,
            at: CGPoint(x: selectedFirstCard.frame.maxX - 8, y: selectedFirstCard.frame.maxY - 0.5),
            in: hostWindow,
            name: "selected update bottom divider"
        )
        let unselectedDivider = try requireScreenshotColor(
            selectedCardScreenshot,
            at: CGPoint(x: unselectedSecondCard.frame.maxX - 8, y: unselectedSecondCard.frame.maxY - 0.5),
            in: hostWindow,
            name: "unselected update bottom divider"
        )
        XCTAssertLessThan(rgbDistance(selectedDivider, unselectedDivider), 36,
                          "selected cards should keep the theme divider color at the bottom edge")
        XCTAssertGreaterThan(rgbDistance(selectedDivider, railInner), 36,
                             "the selected bottom divider should remain distinct from the primary rail")
        let mouseOpenAreaEdge = try requireScreenshotColor(
            selectedCardScreenshot,
            at: CGPoint(x: openFirst.frame.minX + 12, y: openFirst.frame.minY + 0.5),
            in: hostWindow,
            name: "selected update open area top edge after mouse input"
        )
        let mouseOpenAreaInterior = try requireScreenshotColor(
            selectedCardScreenshot,
            at: CGPoint(x: openFirst.frame.minX + 12, y: openFirst.frame.minY + 2.5),
            in: hostWindow,
            name: "selected update open area interior after mouse input"
        )
        XCTAssertLessThan(rgbDistance(mouseOpenAreaEdge, mouseOpenAreaInterior), 24,
                          "mouse selection should not paint a focus outline on the open area")
        app.typeKey(XCUIKeyboardKey.space.rawValue, modifierFlags: [])
        let keyboardCardScreenshot = hostWindow.screenshot()
        let keyboardOpenAreaEdge = try requireScreenshotColor(
            keyboardCardScreenshot,
            at: CGPoint(x: openFirst.frame.minX + 12, y: openFirst.frame.minY + 0.5),
            in: hostWindow,
            name: "selected update open area top edge after keyboard activation"
        )
        let keyboardOpenAreaInterior = try requireScreenshotColor(
            keyboardCardScreenshot,
            at: CGPoint(x: openFirst.frame.minX + 12, y: openFirst.frame.minY + 2.5),
            in: hostWindow,
            name: "selected update open area interior after keyboard activation"
        )
        XCTAssertGreaterThan(rgbDistance(keyboardOpenAreaEdge, keyboardOpenAreaInterior), 36,
                             "keyboard focus should paint a visible outline on the open area")
        openFirst.click()
        XCTAssertEqual(unselectedSecondCard.frame.width, secondCardFrameBeforeClick.width, accuracy: 1.0,
                       "the unselected card should retain its width before selection")
        XCTAssertEqual(unselectedSecondCard.frame.height, secondCardFrameBeforeClick.height, accuracy: 1.0,
                       "the unselected card should retain its height before selection")

        XCTAssertTrue(
            waitForSemanticText("DESK-184 local updates", in: firstReadingPane),
            "opening the first update should select its local DESK-184 reading pane"
        )
        XCTAssertTrue(
            firstReadingPane.label == "DESK-184 local updates"
                || firstReadingPane.title == "DESK-184 local updates",
            "the selected reading pane should expose its exact local update identity"
        )
        XCTAssertTrue(updateList.exists, "opening an update should keep the Focus inbox list visible")
        assertFiniteBounded(firstReadingPane.frame, name: "DESK-184 update-reading-pane")
        XCTAssertTrue(hostWindow.frame.contains(firstReadingPane.frame), "reading pane should remain inside the workspace")
        XCTAssertLessThanOrEqual(
            updateList.frame.maxX,
            firstReadingPane.frame.minX + 2,
            "Focus inbox list and reading pane should not overlap"
        )
        let metadata = try require(app.descendants(matching: .any)["update-metadata-0"], "update-metadata-0")
        assertFiniteBounded(metadata.frame, name: "first compact activity row")
        let groupCount = try require(
            app.descendants(matching: .any)["update-group-count-0"],
            "update-group-count-0"
        )
        XCTAssertTrue(semanticText(groupCount).contains("1"), "DESK-184 should expose its grouped activity event count")
        let expandGroup = try require(
            app.buttons["update-group-expand-0"],
            "update-group-expand-0"
        )
        XCTAssertTrue(firstCard.frame.contains(groupCount.frame), "group count should stay inside the compact ticket header")
        XCTAssertTrue(firstCard.frame.contains(expandGroup.frame), "group expansion control should stay inside the compact ticket header")
        let collapsedHeight = firstCard.frame.height
        expandGroup.click()
        let firstEvent = try require(
            app.descendants(matching: .any)["update-row-0-0"],
            "first DESK-184 activity row after expansion"
        )
        XCTAssertTrue(semanticText(firstEvent).contains("Status:"), "expanded row should expose the fixture status change")
        let expandedCard = try require(app.descendants(matching: .any)["update-card-0"], "expanded DESK-184 group")
        XCTAssertGreaterThan(expandedCard.frame.height, collapsedHeight + 8, "expansion should reveal a visibly taller activity group")
        let collapseGroup = try require(app.buttons["update-group-expand-0"], "collapse DESK-184 activity group")
        collapseGroup.click()
        XCTAssertTrue(
            waitForAbsence(app.descendants(matching: .any)["update-row-0-0"]),
            "collapsing a ticket should hide its event detail row"
        )
        XCTAssertTrue(firstCard.exists, "expanding a ticket should keep its grouped inbox header visible")

        let issueOverview = try require(
            app.descendants(matching: .any)["issue-detail"],
            "shared issue overview for DESK-184"
        )
        XCTAssertTrue(
            issueOverview.label == "Issue detail for DESK-184"
                || issueOverview.title == "Issue detail for DESK-184",
            "the shared inbox overview should expose its exact issue identity"
        )
        let watchAction = try require(app.buttons["issue-watch-button"], "read-only Watch action")
        XCTAssertTrue(semanticText(watchAction).contains("Watch"), "the read-only overview should expose its watch navigation action")
        assertFiniteBounded(watchAction.frame, name: "read-only Watch action")
        let readingDescription = try require(
            app.descendants(matching: .any)["issue-detail-description"],
            "DESK-184 shared issue description"
        )
        assertFiniteBounded(readingDescription.frame, name: "DESK-184 shared Description section")
        XCTAssertTrue(
            semanticText(readingDescription).contains("Poll Jira incrementally"),
            "shared issue Description section should expose cached issue description text"
        )
        let linkedIssues = try require(
            app.descendants(matching: .any)["issue-detail-linked-issues"],
            "DESK-184 shared issue linked issues"
        )
        assertFiniteBounded(linkedIssues.frame, name: "DESK-184 shared issue linked issues")
        XCTAssertTrue(semanticText(linkedIssues).contains("Linked issues"), "shared overview should expose its linked issues section")
        let linkedIssueKey = try require(
            app.buttons["issue-detail-linked-key-0"],
            "first DESK-184 linked issue key"
        )
        XCTAssertEqual(linkedIssueKey.label.isEmpty ? linkedIssueKey.title : linkedIssueKey.label, "DESK-179")
        let activityTimeline = try require(
            app.descendants(matching: .any)["update-reading-activity"],
            "DESK-184 local activity timeline"
        )
        assertFiniteBounded(activityTimeline.frame, name: "DESK-184 local activity timeline")
        _ = try require(
            app.descendants(matching: .any)["update-reading-activity-event-0"],
            "first DESK-184 local activity event"
        )
        let issueActions = try require(
            app.descendants(matching: .any)["issue-detail-action-bar"],
            "read-only issue action bar"
        )
        for identifier in ["issue-detail-add-comment", "issue-detail-change-status"] {
            let action = try require(app.buttons[identifier], identifier)
            assertFiniteBounded(action.frame, name: identifier)
            XCTAssertTrue(issueActions.frame.contains(action.frame), "\(identifier) should remain inside the action bar")
        }

        // Virtualized feeds must expose stable semantic identities for the rows currently in the
        // viewport and keep every realized row inside the scrolling surface. This assertion is
        // intentionally count-agnostic so it remains valid as the fixture grows beyond five rows.
        let rows = app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "update-metadata-")
        )
        XCTAssertGreaterThan(rows.count, 0, "updates should expose at least one realized semantic row")
        var priorMaxY: CGFloat?
        for index in 0..<rows.count {
            let row = try require(rows.element(boundBy: index), "update-metadata-\(index)")
            XCTAssertFalse(semanticText(row).isEmpty, "realized update row \(index) should expose identity")
            assertFiniteBounded(row.frame, name: "update-metadata-\(index)")
            XCTAssertTrue(updateList.frame.contains(row.frame), "realized update row \(index) should stay in update list")
            if let priorMaxY {
                XCTAssertGreaterThanOrEqual(row.frame.minY, priorMaxY - 1, "realized update rows should not overlap")
            }
            priorMaxY = row.frame.maxY
        }

        openSecond.click()
        let selectedSecondCard = try require(
            app.descendants(matching: .any)["update-card-1"],
            "selected update-card-1"
        )
        XCTAssertEqual(selectedSecondCard.frame.width, secondCardFrameBeforeClick.width, accuracy: 1.0,
                       "selecting an update should not change the card width")
        XCTAssertEqual(selectedSecondCard.frame.height, secondCardFrameBeforeClick.height, accuracy: 1.0,
                       "selecting an update should not change the card height")
        XCTAssertEqual(firstCard.frame.width, firstCardFrameBeforeClick.width, accuracy: 1.0,
                       "changing selection should not change the first card width")
        XCTAssertEqual(firstCard.frame.height, firstCardFrameBeforeClick.height, accuracy: 1.0,
                       "changing selection should not change the first card height")
        let secondReadingPane = try require(
            app.groups["update-reading-pane"],
            "update-reading-pane for DESK-179"
        )
        XCTAssertTrue(
            waitForSemanticText("DESK-179", in: secondReadingPane),
            "opening the second update should select DESK-179 in the local reading pane"
        )
        XCTAssertTrue(updateList.exists, "selecting DESK-179 should keep the Focus inbox list visible")
        assertFiniteBounded(secondReadingPane.frame, name: "DESK-179 update-reading-pane")
        XCTAssertTrue(hostWindow.frame.contains(secondReadingPane.frame), "DESK-179 reading pane should stay inside the workspace")
        XCTAssertLessThanOrEqual(
            updateList.frame.maxX,
            secondReadingPane.frame.minX + 2,
            "Focus inbox list and DESK-179 reading pane should not overlap"
        )

        let readingPaneScreenshot = XCTAttachment(screenshot: app.screenshot())
        readingPaneScreenshot.name = "updates-focus-inbox-DESK-179-reading-pane"
        readingPaneScreenshot.lifetime = .keepAlways
        add(readingPaneScreenshot)

        let openFullDetails = try require(
            app.buttons["update-open-full-details"],
            "update-open-full-details"
        )
        XCTAssertTrue(secondReadingPane.frame.contains(openFullDetails.frame), "full-details action should stay inside the reading pane")
        openFullDetails.click()

        let detail = try require(
            app.descendants(matching: .any)["issue-detail"],
            "issue-detail after opening DESK-179 full details"
        )
        XCTAssertTrue(
            waitForSemanticText("Issue detail for DESK-179", in: detail),
            "full details should navigate to the established DESK-179 Issues detail"
        )
        assertFiniteBounded(detail.frame, name: "issue-detail for DESK-179")
        XCTAssertTrue(hostWindow.frame.contains(detail.frame), "DESK-179 issue detail should remain inside the workspace")
        XCTAssertTrue(waitForAbsence(app.descendants(matching: .any)["update-list"]), "full details should leave the Focus inbox")
    }

    func testIssueWatch() throws {
        try launchFixture(scenario: "issue-watch")

        let detail = try require(
            app.descendants(matching: .any)["issue-detail"],
            "issue detail in inert watch fixture"
        )
        XCTAssertTrue(
            waitForSemanticText("Issue detail for DESK-184", in: detail),
            "watch fixture should retain its selected issue identity"
        )
        let watch = try require(app.buttons["issue-watch-button"], "issue-watch-button")
        XCTAssertTrue(watch.isEnabled, "known local watch state should allow opening confirmation")
        assertFiniteBounded(watch.frame, name: "issue-watch-button")
        watch.click()

        let confirmation = try require(
            app.descendants(matching: .any)["issue-watch-confirmation"],
            "issue-watch-confirmation"
        )
        XCTAssertTrue(semanticText(confirmation).localizedCaseInsensitiveContains("watch"))
        XCTAssertTrue(semanticText(confirmation).contains("DESK-184"), "confirmation should identify the exact issue")
        assertFiniteBounded(confirmation.frame, name: "issue-watch-confirmation")
        XCTAssertTrue(detail.frame.contains(confirmation.frame), "watch confirmation should stay inside issue detail")

        let confirm = try require(app.buttons["issue-watch-confirm"], "issue-watch-confirm")
        let cancel = try require(app.buttons["issue-watch-cancel"], "issue-watch-cancel")
        assertFiniteBounded(confirm.frame, name: "issue-watch-confirm")
        assertFiniteBounded(cancel.frame, name: "issue-watch-cancel")
        XCTAssertTrue(cancel.isEnabled, "Cancel should remain available in the confirmation state")
        XCTAssertTrue(confirmation.frame.contains(confirm.frame), "Confirm should remain inside its confirmation surface")
        XCTAssertTrue(confirmation.frame.contains(cancel.frame), "Cancel should remain inside its confirmation surface")

        // GPUI's current AccessKit bridge does not expose `aria-disabled`, so XCTest reports the
        // disabled button as enabled. Verify pointer activation stays inert and preserves state.
        confirm.click()
        XCTAssertTrue(confirmation.exists, "Confirm without a writer must not dismiss the prompt")
        XCTAssertFalse(
            app.descendants(matching: .any)["issue-watch-feedback"].exists,
            "Confirm without a writer must not report a write result"
        )
        XCTAssertEqual(watch.label.isEmpty ? watch.title : watch.label, "Watch")

        cancel.click()
        XCTAssertTrue(
            waitForAbsence(app.descendants(matching: .any)["issue-watch-confirmation"]),
            "Cancel should dismiss confirmation without changing the watch state"
        )
        XCTAssertTrue(watch.exists, "cancelling should retain the issue Watch action")
        XCTAssertEqual(watch.label.isEmpty ? watch.title : watch.label, "Watch")
        XCTAssertFalse(
            app.descendants(matching: .any)["issue-watch-feedback"].exists,
            "cancelling should not show a write result"
        )
    }

    func testTeam() throws {
        try launchFixture(scenario: "team")
        let team = try require(app.descendants(matching: .any)["nav-team"], "nav-team")
        team.click()
        let table = try require(app.descendants(matching: .any)["team-table"], "team-table")
        let detail = try require(
            app.descendants(matching: .any)["issue-detail-description"],
            "issue-detail-description"
        )
        let detailSemanticText = [detail.label, detail.title, detail.value as? String ?? ""]
            .joined(separator: " ")
        XCTAssertTrue(
            detailSemanticText.contains("Cached Team Tracker detail"),
            "fixture Team Tracker selection should expose its cached description"
        )
        let detailLoading = app.descendants(matching: .any)["issue-detail-loading"]
        XCTAssertFalse(detailLoading.exists, "cached Team Tracker detail should not show a spinner")
        let hostWindow = try require(app.windows.firstMatch, "fixture host window")
        XCTAssertGreaterThan(table.frame.width, 560, "dense Team Tracker table should retain its 596 px budget")
        XCTAssertLessThanOrEqual(table.frame.width, 620, "dense Team Tracker table should remain bounded")
        XCTAssertGreaterThan(detail.frame.width, 260, "Team Tracker detail should retain a usable width")
        XCTAssertTrue(hostWindow.frame.contains(table.frame), "Team Tracker table should remain inside the host window")
        XCTAssertTrue(hostWindow.frame.contains(detail.frame), "Team Tracker detail should remain inside the host window")
        XCTAssertTrue(table.frame.maxX <= detail.frame.minX + 2, "Team Tracker panes should not overlap")

        let expectedRows = [
            (key: "DESK-171", summary: "Read-only Jira desktop MVP", assignee: "Amina Yusuf", age: "2d"),
            (key: "DESK-184", summary: "Surface Jira update notifications in the desktop feed", assignee: "Amina Yusuf", age: "1d"),
        ]
        let compactCells = app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "team-ticket-cell-")
        )
        for expected in expectedRows {
            let keyCell = try require(
                app.descendants(matching: .any)["team-ticket-key-\(expected.key)"],
                "team-ticket-key-\(expected.key)"
            )
            var rowCells: [XCUIElement] = []
            for index in 0..<compactCells.count {
                let cell = compactCells.element(boundBy: index)
                if abs(cell.frame.midY - keyCell.frame.midY) <= 2 {
                    rowCells.append(cell)
                }
            }
            rowCells.sort { $0.frame.minX < $1.frame.minX }
            XCTAssertEqual(rowCells.count, 3, "compact row should expose Summary, Assignee, and Age for \(expected.key)")
            guard rowCells.count == 3 else { continue }

            XCTAssertTrue(semanticText(keyCell).contains(expected.key), "compact table should expose ticket key \(expected.key)")
            XCTAssertTrue(semanticText(rowCells[0]).contains(expected.summary), "compact table should expose summary for \(expected.key)")
            XCTAssertTrue(semanticText(rowCells[1]).contains(expected.assignee), "compact table should expose assignee for \(expected.key)")
            XCTAssertTrue(semanticText(rowCells[2]).contains(expected.age), "compact table should expose age for \(expected.key)")
            XCTAssertEqual(rowCells[0].frame.minX - keyCell.frame.minX, 96, accuracy: 3, "Ticket should retain its 96 px compact column")
            XCTAssertEqual(rowCells[1].frame.minX - rowCells[0].frame.minX, 270, accuracy: 3, "Summary should retain its 270 px compact column")
            XCTAssertEqual(rowCells[2].frame.minX - rowCells[1].frame.minX, 150, accuracy: 3, "Assignee should retain its 150 px compact column")
            XCTAssertEqual(rowCells[2].frame.width + 16, 80, accuracy: 3, "Age content plus cell padding should retain its 80 px compact column")
            for cell in [keyCell] + rowCells {
                XCTAssertTrue(table.frame.contains(cell.frame), "compact cell should remain inside the table for \(expected.key)")
            }
            for index in 1..<rowCells.count {
                XCTAssertFalse(rowCells[index - 1].frame.intersects(rowCells[index].frame), "compact cells must not overlap for \(expected.key)")
            }
        }

        let teamHeader = try require(app.groups["team-header"], "team-header")
        XCTAssertTrue(
            semanticText(teamHeader).contains("2 in-progress tickets displayed · 2 configured team members"),
            "Team header should expose the semantic displayed-ticket and configured-member counts"
        )
        assertFiniteBounded(teamHeader.frame, name: "team-header")
        XCTAssertLessThan(teamHeader.frame.height, 120, "Team header should remain a compact band")
        XCTAssertTrue(hostWindow.frame.contains(teamHeader.frame), "Team header should remain inside the host window")

        let configureTeam = try require(app.buttons["configure-team-header"], "configure-team-header")
        assertFiniteBounded(configureTeam.frame, name: "configure-team-header")
        XCTAssertLessThan(configureTeam.frame.height, 80, "Configure Team action should retain a bounded height")
        XCTAssertTrue(teamHeader.frame.contains(configureTeam.frame), "Configure Team action should remain inside the Team header")
        configureTeam.click()

        _ = try require(
            app.descendants(matching: .any)["settings-content-team-tracker"],
            "settings-content-team-tracker from Team header"
        )
        let teamTrackerCategory = try require(
            app.descendants(matching: .tab)["settings-category-team-tracker"],
            "settings-category-team-tracker from Team header"
        )
        XCTAssertTrue(
            waitForAXValue(["true", "1"], in: teamTrackerCategory),
            "Team header configuration should select the Team tracker Settings category"
        )
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

    private func semanticText(_ element: XCUIElement) -> String {
        [element.label, element.title, axValue(element)].joined(separator: " ")
    }

    private func assertFiniteBounded(_ frame: CGRect, name: String) {
        XCTAssertTrue(frame.minX.isFinite && frame.maxX.isFinite && frame.minY.isFinite && frame.maxY.isFinite,
                      "\(name) frame should remain finite")
        XCTAssertGreaterThan(frame.width, 0, "\(name) should have visible width")
        XCTAssertGreaterThan(frame.height, 0, "\(name) should have visible height")
        XCTAssertLessThan(frame.width, 2_000, "\(name) width should remain bounded")
        XCTAssertLessThan(frame.height, 1_000, "\(name) height should remain bounded")
    }

    private func requireScreenshotColor(
        _ screenshot: XCUIScreenshot,
        at point: CGPoint,
        in window: XCUIElement,
        name: String
    ) throws -> NSColor {
        guard let cgImage = screenshot.image.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
            XCTFail("Could not read \(name) from the fixture screenshot")
            throw NSError(domain: "JiraDeskUITests", code: 6)
        }
        let bitmap = NSBitmapImageRep(cgImage: cgImage)
        let windowFrame = window.frame
        let x = Int((point.x - windowFrame.minX) * CGFloat(bitmap.pixelsWide) / windowFrame.width)
        let y = Int((point.y - windowFrame.minY) * CGFloat(bitmap.pixelsHigh) / windowFrame.height)
        guard x >= 0, x < bitmap.pixelsWide, y >= 0, y < bitmap.pixelsHigh,
              let color = bitmap.colorAt(x: x, y: y) else {
            XCTFail("\(name) fell outside the fixture screenshot")
            throw NSError(domain: "JiraDeskUITests", code: 7)
        }
        return color.usingColorSpace(.deviceRGB) ?? color
    }

    private func rgbDistance(_ lhs: NSColor, _ rhs: NSColor) -> CGFloat {
        let left = lhs.usingColorSpace(.deviceRGB) ?? lhs
        let right = rhs.usingColorSpace(.deviceRGB) ?? rhs
        return abs(left.redComponent - right.redComponent) * 255
            + abs(left.greenComponent - right.greenComponent) * 255
            + abs(left.blueComponent - right.blueComponent) * 255
    }

    private func waitForSemanticText(_ expected: String, in element: XCUIElement) -> Bool {
        let expectation = XCTNSPredicateExpectation(
            predicate: NSPredicate { object, _ in
                guard let element = object as? XCUIElement else {
                    return false
                }
                return self.semanticText(element).contains(expected)
            },
            object: element
        )
        return XCTWaiter.wait(for: [expectation], timeout: 8) == .completed
    }

    private func waitForAXValue(_ expected: Set<String>, in element: XCUIElement) -> Bool {
        let expectation = XCTNSPredicateExpectation(
            predicate: NSPredicate { object, _ in
                guard let element = object as? XCUIElement else {
                    return false
                }
                return expected.contains(self.axValue(element).lowercased())
            },
            object: element
        )
        return XCTWaiter.wait(for: [expectation], timeout: 8) == .completed
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
