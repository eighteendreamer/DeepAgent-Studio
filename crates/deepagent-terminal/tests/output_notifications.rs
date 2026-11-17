use deepagent_terminal::TerminalOutputSignal;
use std::time::Duration;

#[tokio::test(start_paused = true)]
async fn subscribing_after_output_observes_the_current_revision_and_idle_wait_is_bounded() {
    let signal = TerminalOutputSignal::default();
    signal.notify();
    let mut subscription = signal.subscribe();
    let notice = subscription.wait_after(0, Duration::from_secs(5)).await;
    assert_eq!(notice.revision, 1);
    assert!(!notice.closed);
    let start = tokio::time::Instant::now();
    let quiet = subscription
        .wait_after(notice.revision, Duration::from_secs(5))
        .await;
    assert_eq!(quiet.revision, notice.revision);
    assert!(!quiet.closed);
    assert_eq!(start.elapsed(), Duration::from_secs(5));
}

#[tokio::test(start_paused = true)]
async fn output_wakes_an_inflight_wait_and_many_notices_are_coalesced_for_each_observer() {
    let signal = TerminalOutputSignal::default();
    let mut first = signal.subscribe();
    let mut second = signal.subscribe();
    let wait = tokio::spawn(async move { first.wait_after(0, Duration::from_secs(5)).await });
    tokio::task::yield_now().await;
    for _ in 0..1000 {
        signal.notify();
    }
    assert_eq!(wait.await.unwrap().revision, 1000);
    assert_eq!(
        second.wait_after(0, Duration::from_secs(5)).await.revision,
        1000
    );
}

#[tokio::test(start_paused = true)]
async fn notifier_drop_wakes_waiters_and_reports_closed_without_replaying_output() {
    let signal = TerminalOutputSignal::default();
    let mut subscription = signal.subscribe();
    let wait =
        tokio::spawn(async move { subscription.wait_after(0, Duration::from_secs(5)).await });
    tokio::task::yield_now().await;
    drop(signal);
    let notice = wait.await.unwrap();
    assert!(notice.closed);
    assert_eq!(notice.revision, 0);
    let payload = serde_json::to_value(notice).unwrap();
    assert_eq!(
        payload.as_object().unwrap().len(),
        2,
        "notice is not a second output stream"
    );
}
