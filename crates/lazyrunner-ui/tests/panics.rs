//! The panic hook is process-wide: its test has a test binary of its own, so it can't swallow other tests' panics.

use lazyrunner_ui::panics;

#[test]
fn a_panic_in_another_thread_is_kept_for_the_log_not_printed() {
    panics::keep_off_screen();
    let joined =
        std::thread::Builder::new().name("checks".into()).spawn(|| panic!("the port list was empty")).unwrap().join();
    assert!(joined.is_err(), "the thread still ends");
    let kept = panics::take();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(kept[0].starts_with("checks: the port list was empty ("), "{kept:?}");
    assert!(panics::take().is_empty(), "taken once");
}
