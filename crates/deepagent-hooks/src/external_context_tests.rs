use super::*;

fn report(value: &str, code: i32, event: HookEvent) -> HookReport {
    interpret_report_for_event(
        &HookCommandResult {
            stdout: value.into(),
            stderr: String::new(),
            exit_code: code,
        },
        event,
    )
}

#[test]
fn h140_context_parser_is_event_success_and_protocol_scoped() {
    let context = r#"{"permissionDecision":"allow","hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"accepted"}}"#;
    assert_eq!(
        report(context, 0, HookEvent::UserPromptSubmit).additional_context[0].text,
        "accepted"
    );
    for (text, code, event) in [
        (context, 7, HookEvent::UserPromptSubmit),
        (context, 0, HookEvent::SessionStart),
        (
            r#"{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"no"}}"#,
            0,
            HookEvent::UserPromptSubmit,
        ),
        (
            r#"{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":42}}"#,
            0,
            HookEvent::UserPromptSubmit,
        ),
        (
            r#"{"hookSpecificOutput":{"additionalContext":"no"}}"#,
            0,
            HookEvent::UserPromptSubmit,
        ),
        (r#"{"debug":"diagnostic"}"#, 0, HookEvent::UserPromptSubmit),
        ("{broken", 0, HookEvent::UserPromptSubmit),
        (
            r#"{"async":true,"asyncTimeout":5,"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"no"}}"#,
            0,
            HookEvent::UserPromptSubmit,
        ),
    ] {
        assert!(
            report(text, code, event).additional_context.is_empty(),
            "{text}"
        );
    }
    let plain = report(" plain context\n", 0, HookEvent::UserPromptSubmit);
    assert_eq!(plain.additional_context[0].text, "plain context");
    let deny = report(
        r#"{"decision":"block","hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"not yet admitted"}}"#,
        0,
        HookEvent::UserPromptSubmit,
    );
    assert!(deny.outcome.is_deny());
    // A per-handler report can carry both; registry admission must discard it
    // on aggregate denial, rather than allowing parsing to erase the decision.
    assert_eq!(deny.additional_context.len(), 1);
}
