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

#[test]
fn h111_precompact_context_requires_success_matching_event_and_synchronous_output() {
    let context = r#"{"hookSpecificOutput":{"hookEventName":"PreCompact","additionalContext":"keep synthetic fact"}}"#;
    assert_eq!(
        report(context, 0, HookEvent::PreCompact).additional_context[0].text,
        "keep synthetic fact"
    );
    assert_eq!(
        report("plain instructions", 0, HookEvent::PreCompact).additional_context[0].text,
        "plain instructions"
    );
    for (text, code) in [
        (context, 7),
        (context, 2),
        ("  ", 0),
        (
            r#"{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"wrong event"}}"#,
            0,
        ),
        (
            r#"{"async":true,"hookSpecificOutput":{"hookEventName":"PreCompact","additionalContext":"late"}}"#,
            0,
        ),
        (
            r#"{"hookSpecificOutput":{"hookEventName":"PreCompact","additionalContext":42}}"#,
            0,
        ),
        (r#"{"debug":"diagnostic"}"#, 0),
        ("{malformed", 0),
    ] {
        assert!(
            report(text, code, HookEvent::PreCompact)
                .additional_context
                .is_empty(),
            "{text}"
        );
    }
}
