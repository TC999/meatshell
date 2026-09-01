use crate::app::launch::parse;

#[test]
fn no_flags_means_plain_launch() {
    let intent = parse(&["meatshell".to_string()]);
    assert!(!intent.new_window);
    assert!(intent.directory.is_none());
}

#[test]
fn new_window_flag_is_recognised() {
    let intent = parse(&["meatshell".to_string(), "--new-window".to_string()]);
    assert!(intent.new_window);
    assert!(intent.directory.is_none());
}

#[test]
fn unrelated_args_do_not_trigger_new_window() {
    let intent = parse(&["meatshell".to_string(), "--version".to_string()]);
    assert!(!intent.new_window);
    assert!(intent.directory.is_none());
}

#[test]
fn dir_flag_is_recognised() {
    let intent = parse(&[
        "meatshell".to_string(),
        "--new-window".to_string(),
        "--dir".to_string(),
        r"C:\Users\test".to_string(),
    ]);
    assert!(intent.new_window);
    assert_eq!(intent.directory.as_deref(), Some(r"C:\Users\test"));
}

#[test]
fn dir_flag_standalone() {
    let intent = parse(&[
        "meatshell".to_string(),
        "--dir".to_string(),
        "/home/user/projects".to_string(),
    ]);
    assert!(!intent.new_window);
    assert_eq!(intent.directory.as_deref(), Some("/home/user/projects"));
}
