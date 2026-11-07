use super::*;

#[test]
fn rendering_an_unchanged_document_reproduces_it_byte_for_byte() {
    let cases = [
        "",
        "a=1",
        "a=1\n",
        "#Minecraft server properties\n#Sat Nov 07 15:00:00 UTC 2026\nmotd=A Minecraft Server\n",
        "a = 1\r\nb:2\r\n\r\n! bang comment\r\n",
        "  indented=yes\n\tkey\tvalue\nweird\\=key=v\n",
        "multi=one \\\n    two \\\n three\nnext=1\n",
        "trailing=\\\n",
        "no-newline-at-end=1",
        "mixed=1\r\nline=2\nendings=3\r",
        "key-only\nempty=\n=value-without-key\n",
    ];
    for text in cases {
        assert_eq!(PropertiesDoc::parse(text).render(), text, "{text:?}");
    }
}

#[test]
fn parses_separators_like_java() {
    let doc = PropertiesDoc::parse("a=1\nb:2\nc 3\nd = 4\ne\t:\t5\nf==6\ng\n  h   =   spaced value  \n");
    let expected = [
        ("a", "1"),
        ("b", "2"),
        ("c", "3"),
        ("d", "4"),
        ("e", "5"),
        ("f", "=6"),
        ("g", ""),
        ("h", "spaced value  "),
    ];
    assert_eq!(doc.entries(), expected);
}

#[test]
fn understands_escapes_and_continuations() {
    let text = "path=C\\:\\\\Games\\\\mc\nmotd=\\u00a7aHello\\u0021\\tTab\nsmile=\\ud83d\\ude00\nspaced\\ key=v\nlong=first,\\\n     second,\\\n\tthird\nliteral=\\q\n";
    let doc = PropertiesDoc::parse(text);
    assert_eq!(doc.get("path"), Some("C:\\Games\\mc"));
    assert_eq!(doc.get("motd"), Some("\u{a7}aHello!\tTab"));
    assert_eq!(doc.get("smile"), Some("\u{1f600}"));
    assert_eq!(doc.get("spaced key"), Some("v"));
    assert_eq!(doc.get("long"), Some("first,second,third"));
    assert_eq!(doc.get("literal"), Some("q"));
    assert_eq!(doc.render(), text);
}

#[test]
fn an_even_number_of_backslashes_does_not_continue() {
    let doc = PropertiesDoc::parse("a=ends with backslash\\\\\nb=2\n");
    assert_eq!(doc.get("a"), Some("ends with backslash\\"));
    assert_eq!(doc.get("b"), Some("2"));
}

#[test]
fn malformed_unicode_escapes_are_kept_rather_than_failing() {
    let doc = PropertiesDoc::parse("a=\\u12\nb=\\uZZZZ\n");
    assert_eq!(doc.get("a"), Some("\\u12"));
    assert_eq!(doc.get("b"), Some("\\uZZZZ"));
}

#[test]
fn the_last_duplicate_wins_and_is_the_one_edited() {
    let mut doc = PropertiesDoc::parse("a=1\nb=x\na=2\n");
    assert_eq!(doc.get("a"), Some("2"));
    assert_eq!(doc.entries(), [("a", "2"), ("b", "x")]);
    doc.set("a", "3");
    assert_eq!(doc.render(), "a=1\nb=x\na=3\n");
}

#[test]
fn editing_one_value_leaves_every_other_line_alone() {
    let text = "#Minecraft server properties\n#Sat Nov 07 2026\nmotd = Old  \n# my note\nmax-players=20\r\nlong=a\\\n  b\n";
    let mut doc = PropertiesDoc::parse(text);
    doc.set("max-players", "50");
    assert_eq!(
        doc.render(),
        "#Minecraft server properties\n#Sat Nov 07 2026\nmotd = Old  \n# my note\nmax-players=50\r\nlong=a\\\n  b\n",
        "every line keeps its own ending"
    );
    let mut crlf = PropertiesDoc::parse("a=1\r\n");
    crlf.set("b", "2");
    assert_eq!(
        crlf.render(),
        "a=1\r\nb=2\r\n",
        "added lines follow the file's CRLF"
    );

    let mut lf = PropertiesDoc::parse("a=1\n# keep\nmotd = Old\n");
    lf.set("motd", "New");
    assert_eq!(
        lf.render(),
        "a=1\n# keep\nmotd = New\n",
        "the original separator is kept"
    );
}

#[test]
fn setting_the_same_value_keeps_the_original_formatting() {
    let text = "motd=\\u00a7aHi\n";
    let mut doc = PropertiesDoc::parse(text);
    doc.set("motd", "\u{a7}aHi");
    assert_eq!(doc.render(), text);
}

#[test]
fn new_keys_are_appended_and_a_final_newline_added() {
    let mut doc = PropertiesDoc::parse("a=1");
    doc.set("b", "2");
    assert_eq!(doc.render(), "a=1\nb=2\n");
    let mut empty = PropertiesDoc::parse("");
    empty.set("server-port", "25565");
    assert_eq!(empty.render(), "server-port=25565\n");
}

#[test]
fn removes_every_occurrence() {
    let mut doc = PropertiesDoc::parse("a=1\nb=2\na=3\n");
    doc.remove("a");
    assert_eq!(doc.render(), "b=2\n");
    assert_eq!(doc.get("a"), None);
    doc.remove("missing");
    assert_eq!(doc.render(), "b=2\n");
}

#[test]
fn decodes_utf8_and_falls_back_to_latin1() {
    let utf8 = PropertiesDoc::from_bytes("motd=Caf\u{e9} \u{2713}\n".as_bytes());
    assert_eq!(utf8.get("motd"), Some("Caf\u{e9} \u{2713}"));
    let latin1 = PropertiesDoc::from_bytes(b"motd=Caf\xe9\n");
    assert_eq!(latin1.get("motd"), Some("Caf\u{e9}"));
}

/// Whatever is set reads back identically after rendering and parsing again.
#[test]
fn any_value_or_key_round_trips_through_set_render_parse() {
    let samples = [
        "",
        " leading space",
        "trailing space ",
        "a=b:c d",
        "#not a comment",
        "!bang",
        "back\\slash\\",
        "new\nline\r\ncrlf",
        "tab\tand\x0cformfeed",
        "\u{a7}6Gold \u{a7}lBold",
        "emoji \u{1f600} and \u{4e2d}\u{6587}",
        "\\u0041 literal escape text",
        "ends with backslash \\",
    ];
    // Deterministic pseudo-random strings over an alphabet of troublemakers.
    let alphabet: Vec<char> = " =:#!\\\n\r\ta\u{e9}\u{1f600}".chars().collect();
    let mut seed: u32 = 0x2545_f491;
    let mut generated = Vec::new();
    for _ in 0..300 {
        let mut text = String::new();
        for _ in 0..(seed % 12) {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            text.push(alphabet[(seed as usize) % alphabet.len()]);
        }
        generated.push(text);
    }
    for value in samples.iter().map(|s| (*s).to_owned()).chain(generated) {
        let mut doc = PropertiesDoc::parse("# header\nexisting=1\n");
        doc.set("existing", &value);
        doc.set("new-key", &value);
        if !value.is_empty() {
            doc.set(&value, "as a key");
        }
        let reparsed = PropertiesDoc::parse(&doc.render());
        assert_eq!(
            reparsed.get("existing"),
            Some(value.as_str()),
            "{value:?} in\n{}",
            doc.render()
        );
        assert_eq!(reparsed.get("new-key"), Some(value.as_str()), "{value:?}");
        if !value.is_empty() {
            assert_eq!(
                reparsed.get(&value),
                Some("as a key"),
                "key {value:?} in\n{}",
                doc.render()
            );
        }
        assert!(reparsed.render().starts_with("# header\n"));
    }
}
