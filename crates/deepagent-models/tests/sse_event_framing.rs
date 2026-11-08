use deepagent_models::sse::SseParser;

#[test]
fn h30_only_one_stream_leading_bom_is_ignored() {
    let mut parser = SseParser::new();
    assert!(parser.feed("").is_empty());
    assert!(parser.feed("\u{feff}").is_empty());
    assert_eq!(parser.feed("data: first\n\n"), vec!["first"]);
    assert_eq!(parser.feed("data: \u{feff}kept\n\n"), vec!["\u{feff}kept"]);
    let mut parser = SseParser::new();
    assert!(parser.feed("\u{feff}\u{feff}data: ignored\n\n").is_empty());
}

#[test]
fn h30_dispatch_requires_a_blank_line_even_across_crlf_chunks() {
    let mut parser = SseParser::new();
    assert!(parser.feed("data: first\r").is_empty());
    assert!(parser.feed("\ndata: second\r").is_empty());
    assert_eq!(parser.feed("\n\r"), vec!["first\nsecond"]);
    assert!(
        parser.feed("\n").is_empty(),
        "CRLF is one delimiter, not two"
    );
    assert_eq!(parser.feed("data: third\r\r"), vec!["third"]);
}

#[test]
fn h30_every_two_chunk_boundary_preserves_fields_and_unicode() {
    let wire = concat!(
        "\u{feff}: heartbeat\r\n",
        "event: message\rid: ignored\nretry: 10\r\n",
        "data: 中文🙂\r\n",
        ": interleaved comment\n",
        "data:  second\r\n\r\n",
        "data\n\n",
        "data: \u{feff}not-a-leading-bom\r\r",
    );
    let expected = vec!["中文🙂\n second", "", "\u{feff}not-a-leading-bom"];
    let boundaries: Vec<_> = wire
        .char_indices()
        .map(|(i, _)| i)
        .chain([wire.len()])
        .collect();
    for &first in &boundaries {
        for &second in boundaries.iter().filter(|&&second| second >= first) {
            let mut parser = SseParser::new();
            let mut actual = parser.feed(&wire[..first]);
            actual.extend(parser.feed(&wire[first..second]));
            actual.extend(parser.feed(&wire[second..]));
            assert_eq!(actual, expected, "split {first}/{second}");
            assert_eq!(parser.flush(), None);
        }
    }
}

#[test]
fn h30_eof_discards_both_partial_lines_and_undispatched_complete_lines() {
    for tail in [
        "data: tail",
        "data: tail\n",
        "data: tail\r",
        "data: tail\r\n",
        "data: first\ndata: second",
        "data: first\ndata: second\n",
    ] {
        let mut parser = SseParser::new();
        assert!(parser.feed(tail).is_empty(), "unterminated event: {tail:?}");
        assert_eq!(parser.flush(), None);
        assert_eq!(parser.flush(), None);
        assert_eq!(parser.feed("data: next\n\n"), vec!["next"]);
    }
}

#[test]
fn h30_fields_are_literal_and_only_one_ascii_space_is_removed() {
    let mut parser = SseParser::new();
    assert_eq!(
        parser.feed(concat!(
            "\n: comment\nevent: ignored\nid: 7\nretry: 2\nData: ignored\n\n",
            "data\n\ndata:\n\ndata:  two spaces \t\n",
            "data:\ttab\ndata: \n\n",
        )),
        vec!["", "", " two spaces \t\n\ttab\n"]
    );
}
