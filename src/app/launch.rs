#[cfg(test)]
#[path = "../../tests/app/window_management/launch_intent.rs"]
mod launch_intent_tests;

/// What this process launch is supposed to do, parsed from argv before any
/// window exists. `--new-window` is issued by the OS entry points (Windows
/// jump list, context menu, macOS dock menu, Linux desktop action); when an
/// instance is already running it is forwarded over the single-instance
/// socket instead of opening a second process. `directory`, when set, tells
/// the app to open a Local PowerShell session whose working directory is
/// the path the user invoked the entry point on (the Windows Explorer
/// context-menu "在此处打开 Meatshell" verb).
pub struct LaunchIntent {
    pub new_window: bool,
    pub directory: Option<String>,
}

pub fn parse(args: &[String]) -> LaunchIntent {
    let mut directory: Option<String> = None;
    let mut new_window = false;
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--new-window" {
            new_window = true;
        } else if args[i] == "--dir" && i + 1 < args.len() {
            directory = Some(args[i + 1].clone());
            i += 1;
        }
        i += 1;
    }
    LaunchIntent { new_window, directory }
}
