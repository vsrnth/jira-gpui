//! Command-line entry point for the local macOS accessibility automation host.

fn main() {
    if let Err(error) = jira_gpui::ui_automation::run(std::env::args().skip(1)) {
        eprintln!("jira-ui-automation-host: {error:#}");
        std::process::exit(2);
    }
}
