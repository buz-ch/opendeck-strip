use super::document::Document;
use super::examples;
use super::*;
use crate::config::Config;
use serde_json::{Value, json};

const NOW: u64 = examples::NOW_MS;

/// One on-deck record; `since` is its `since_ms` in seconds before [`NOW`].
fn r(state: &str, pane: u32, since_s: u64) -> Value {
    json!({
        "source": "program_status",
        "tab": {"id": 0, "name": null},
        "pane": {"id": pane, "title": format!("pane {pane}")},
        "state": state,
        "app": format!("app{pane}"),
        "since_ms": NOW - since_s * 1000,
        "on_deck": true,
    })
}

/// One current session `work` holding `panes`.
fn doc(panes: Vec<Value>) -> Document {
    sessions(vec![("work", panes)])
}

fn sessions(s: Vec<(&str, Vec<Value>)>) -> Document {
    serde_json::from_value(json!({
        "version": 2,
        "generated_ms": NOW,
        "attention_enabled": true,
        "sessions": s
            .into_iter()
            .map(|(name, panes)| json!({"name": name, "is_current": true, "panes": panes}))
            .collect::<Vec<_>>(),
    }))
    .unwrap()
}

/// `rec` with `fields` merged in (`tab.name`, `pane.title`, ...).
fn patch(mut rec: Value, fields: Value) -> Value {
    for (k, v) in fields.as_object().unwrap() {
        match (rec.get_mut(k), &v) {
            (Some(Value::Object(old)), Value::Object(new)) => old.extend(new.clone()),
            _ => rec[k] = v.clone(),
        }
    }
    rec
}

fn order(doc: &Document) -> Vec<(u32, State)> {
    entries(doc).iter().map(|e| (e.pane, e.state)).collect()
}

fn text(doc: &Document, o: &Options) -> Option<String> {
    render(doc, o, NOW)
}

fn opts(entry: &str) -> Options {
    Options {
        entry: Template::parse(entry, &with_counts(ENTRY_FIELDS)).unwrap(),
        ..Options::default()
    }
}

fn compact_opts(o: Options) -> Options {
    Options {
        mode: Mode::Compact,
        slot: Some("dial2".into()),
        ..o
    }
}

#[test]
fn states_are_ordered_idle_working_done_error_blocked() {
    let d = doc(vec![
        r("working", 1, 0),
        r("error", 2, 0),
        r("idle", 3, 0),
        r("blocked", 4, 0),
        r("done", 5, 0),
    ]);
    assert_eq!(
        order(&d),
        [
            (4, State::Blocked),
            (2, State::Error),
            (5, State::Done),
            (1, State::Working),
            (3, State::Idle)
        ]
    );
    assert!(State::Idle < State::Working && State::Working < State::Done);
    assert!(State::Done < State::Error && State::Error < State::Blocked);
}

#[test]
fn blocked_is_oldest_first_every_other_state_newest_first() {
    let d = doc(vec![
        r("blocked", 1, 10),
        r("blocked", 2, 300),
        r("blocked", 3, 60),
        r("working", 4, 500),
        r("working", 5, 20),
        r("done", 6, 70),
        r("done", 7, 900),
        r("error", 8, 800),
        r("error", 9, 100),
    ]);
    assert_eq!(
        order(&d),
        [
            (2, State::Blocked),
            (3, State::Blocked),
            (1, State::Blocked),
            (9, State::Error),
            (8, State::Error),
            (6, State::Done),
            (7, State::Done),
            (5, State::Working),
            (4, State::Working)
        ]
    );
    let c = counts(&entries(&d));
    assert_eq!((c.needs_you, c.failed, c.done, c.working), (3, 2, 2, 2));
}

#[test]
fn ties_break_on_tab_pane_title_then_session() {
    // Tab 0's panes come before tab 1's whatever their numbers; pane before title.
    let d = doc(vec![
        r("working", 1, 0),
        patch(r("working", 2, 0), json!({"tab": {"id": 1}})),
        r("working", 9, 0),
        patch(r("working", 8, 0), json!({"tab": {"id": 1}})),
        patch(r("working", 3, 0), json!({"pane": {"title": "b"}})),
        patch(r("working", 4, 0), json!({"pane": {"title": "a"}})),
    ]);
    assert_eq!(
        order(&d),
        [
            (1, State::Working),
            (3, State::Working),
            (4, State::Working),
            (9, State::Working),
            (2, State::Working),
            (8, State::Working)
        ]
    );
    // The title only decides between the same pane in the same tab.
    let d = doc(vec![
        patch(r("working", 1, 0), json!({"pane": {"title": "b"}})),
        patch(r("working", 1, 0), json!({"pane": {"title": "a"}})),
    ]);
    let e = entries(&d);
    assert_eq!(e[0].title, "a");
    let d = doc(vec![
        patch(
            r("working", 1, 0),
            json!({"tab": {"id": 1, "name": "zeta"}}),
        ),
        patch(
            r("working", 1, 0),
            json!({"tab": {"id": 0, "name": "alpha"}}),
        ),
    ]);
    // A tab name is not a sort key.
    assert_eq!(entries(&d)[0].tab_name, "alpha");
    let same = |p: u32| {
        patch(
            r("working", 1, 0),
            json!({"pane": {"title": "same"}, "msg": format!("pane {p}")}),
        )
    };
    let d = sessions(vec![
        ("zeta", vec![same(1)]),
        ("alpha", vec![same(2)]),
        ("mid", vec![same(3)]),
    ]);
    let msgs: Vec<_> = entries(&d).iter().filter_map(|e| e.msg.clone()).collect();
    assert_eq!(msgs, ["pane 2", "pane 3", "pane 1"]);
}

#[test]
fn the_sort_is_stable_and_document_order_does_not_decide() {
    // Same state, time, tab, pane, title and session name: the document's
    // order survives.
    let mut a = r("working", 1, 0);
    a["pane"]["title"] = "same".into();
    let mut b = r("working", 1, 0);
    b["pane"]["title"] = "same".into();
    // Only the message tells them apart.
    let mut b = r("working", 1, 0);
    b["pane"]["title"] = "same".into();
    b["msg"] = "second".into();
    a["msg"] = "first".into();
    let o = opts("{msg}");
    assert_eq!(
        text(
            &sessions(vec![("dup", vec![a.clone()]), ("dup", vec![b.clone()])]),
            &o
        )
        .unwrap(),
        "[c=grey]● 0[/c]\\n[c=grey]✗ 0[/c]\\n[c=grey]✓ 0[/c]\\n[c=#55ccff]» 2[/c]|first|second"
    );
    assert_eq!(
        text(&sessions(vec![("dup", vec![b]), ("dup", vec![a])]), &o).unwrap(),
        "[c=grey]● 0[/c]\\n[c=grey]✗ 0[/c]\\n[c=grey]✓ 0[/c]\\n[c=#55ccff]» 2[/c]|second|first"
    );
    // A peer's blocked pane outranks this session's working pane.
    let d = sessions(vec![
        ("work", vec![r("working", 1, 0)]),
        ("peer", vec![r("blocked", 2, 999)]),
    ]);
    assert_eq!(order(&d), [(2, State::Blocked), (1, State::Working)]);
    // Off-deck records are not entries, wherever they sit.
    let mut off = r("blocked", 3, 0);
    off["on_deck"] = false.into();
    let d = doc(vec![off, r("working", 1, 0)]);
    assert_eq!(order(&d), [(1, State::Working)]);
}

#[test]
fn up_to_three_entries_then_two_and_more() {
    let one = doc(vec![r("blocked", 1, 0)]);
    assert_eq!(
        text(&one, &Options::default()).unwrap().split('|').count(),
        2
    );
    let three = doc(vec![r("blocked", 1, 0), r("error", 2, 0), r("done", 3, 0)]);
    let s = text(&three, &Options::default()).unwrap();
    assert_eq!(s.split('|').count(), 4);
    assert!(!s.contains("more"));
    // Hidden: error, done, working: the top hidden state is the failed one.
    let mut five = vec![r("blocked", 1, 300), r("blocked", 2, 0)];
    five.extend([r("error", 3, 0), r("done", 4, 0), r("working", 5, 0)]);
    let s = text(&doc(five), &Options::default()).unwrap();
    let segs: Vec<_> = s.split('|').collect();
    assert_eq!(segs.len(), 4);
    assert_eq!(segs[2], "[b]app2[/b]\\n[c=orange]needs you[/c]");
    assert_eq!(segs[3], "[b]+3 more[/b]\\n[c=red]failed[/c]\\nsee inbox");
}

#[test]
fn more_says_need_you_with_the_top_hidden_states_look() {
    let mut v: Vec<_> = (1..=4).map(|p| r("blocked", p, p as u64)).collect();
    v.push(r("error", 5, 5));
    let s = text(&doc(v), &Options::default()).unwrap();
    assert!(
        s.ends_with("|[b]+3 more[/b]\\n[c=orange]need you[/c]\\nsee inbox"),
        "{s}"
    );
    assert!(
        !s.contains("needs you[/c]\\nsee inbox"),
        "the entries keep their own label: {s}"
    );
    // A custom more template gets the same hidden fields and the counts.
    let o = Options {
        more: Template::parse(
            "{hidden} {state_label} {color} {marker} of {total}",
            &with_counts(MORE_FIELDS),
        )
        .unwrap(),
        ..Options::default()
    };
    let v: Vec<_> = (1..=5).map(|p| r("working", p, p as u64)).collect();
    let s = text(&doc(v), &o).unwrap();
    assert!(s.ends_with("|3 working #55ccff » of 5"), "{s}");
    // Hidden: done, then two working (the done entry sorts first and is shown).
    let o = Options {
        more: Template::parse("{marker}{hidden}", MORE_FIELDS).unwrap(),
        ..Options::default()
    };
    let s = text(
        &doc(vec![
            r("working", 1, 1),
            r("working", 2, 2),
            r("working", 3, 3),
            r("done", 4, 4),
        ]),
        &o,
    )
    .unwrap();
    assert_eq!(
        s,
        "[c=grey]● 0[/c]\\n[c=grey]✗ 0[/c]\\n[c=green]✓ 1[/c]\\n[c=#55ccff]» 3[/c]|[b]app4[/b]\\n[c=green]done[/c]|[b]app1[/b]\\n[c=#55ccff]working[/c]|»2"
    );
    // The two blocked panes are shown; done and working are hidden, so the
    // segment takes the done look: the higher of the hidden states.
    let s = text(
        &doc(vec![
            r("blocked", 1, 1),
            r("blocked", 2, 2),
            r("working", 3, 3),
            r("done", 4, 4),
        ]),
        &o,
    )
    .unwrap();
    assert!(
        s.ends_with("|[b]app1[/b]\\n[c=orange]needs you[/c]|✓2"),
        "{s}"
    );
    assert!(Template::parse("{count}", &with_counts(MORE_FIELDS)).is_err());
}

#[test]
fn idle_template_replaces_the_whole_output() {
    let idle = doc(vec![]);
    let o = Options {
        idle: Template::parse("{total} quiet", COUNT_FIELDS).unwrap(),
        ..Options::default()
    };
    assert_eq!(text(&idle, &o).as_deref(), Some("0 quiet"));
    assert_eq!(
        text(&idle, &compact_opts(o.clone())).as_deref(),
        Some("0 quiet")
    );
    // An idle template never hides entries.
    let busy = doc(vec![r("blocked", 1, 0)]);
    assert_eq!(text(&busy, &o).unwrap().split('|').count(), 2);
    assert_eq!(
        text(&busy, &compact_opts(o)).unwrap(),
        "[c=orange]● 1[/c]\\napp1"
    );
}

#[test]
fn no_idle_template_gives_counts_and_all_clear() {
    let got = text(&doc(vec![]), &Options::default()).unwrap();
    assert_eq!(
        got,
        format!(
            "[c=grey]● 0[/c]\\n[c=grey]✗ 0[/c]\\n[c=grey]✓ 0[/c]\\n[c=grey]» 0[/c]|{DEFAULT_IDLE_STRIP}"
        )
    );
    assert_eq!(
        text(&doc(vec![]), &compact_opts(Options::default())).as_deref(),
        Some("")
    );
}

#[test]
fn blank_when_idle_clears_only_without_an_idle_template() {
    let o = Options {
        blank_when_idle: true,
        ..Options::default()
    };
    assert_eq!(text(&doc(vec![]), &o), None);
    // Compact mode clears the slot the same way.
    assert_eq!(text(&doc(vec![]), &compact_opts(o.clone())), None);
    let o = Options {
        idle: Template::parse("quiet", COUNT_FIELDS).unwrap(),
        ..o
    };
    assert_eq!(text(&doc(vec![]), &o).as_deref(), Some("quiet"));
    assert_eq!(
        text(&doc(vec![]), &compact_opts(o.clone())).as_deref(),
        Some("quiet")
    );
    let busy = doc(vec![r("done", 1, 0)]);
    assert_eq!(
        text(&busy, &o).unwrap(),
        "[c=grey]● 0[/c]\\n[c=grey]✗ 0[/c]\\n[c=green]✓ 1[/c]\\n[c=grey]» 0[/c]|[b]app1[/b]\\n[c=green]done[/c]"
    );
}

#[test]
fn attention_off_wins_over_everything() {
    let mut d = doc(vec![r("blocked", 1, 0)]);
    d.attention_enabled = false;
    assert_eq!(
        text(&d, &Options::default()).as_deref(),
        Some(ATTENTION_OFF)
    );
    let o = Options {
        blank_when_idle: true,
        idle: Template::parse("quiet", COUNT_FIELDS).unwrap(),
        ..Options::default()
    };
    assert_eq!(text(&d, &compact_opts(o)).as_deref(), Some(ATTENTION_OFF));
    assert_eq!(ATTENTION_OFF, "[c=grey]Zellij attention is off[/c]");
}

#[test]
fn compact_shows_the_first_entry_or_nothing() {
    let o = compact_opts(Options::default());
    assert_eq!(o.slot.as_deref(), Some("dial2"));
    let d = doc(vec![
        r("blocked", 1, 0),
        r("working", 2, 0),
        r("done", 3, 0),
    ]);
    assert_eq!(text(&d, &o).as_deref(), Some("[c=orange]● 1[/c]\\napp1"));
    // The default compact template.
    // A custom compact template; `entry` is not used in compact mode. The
    // count is that of the first entry's state.
    assert_eq!(
        text(&d, &compact_opts(opts("{msg}"))).as_deref(),
        Some("[c=orange]● 1[/c]\\napp1")
    );
    let compact = |t: &str| Template::parse(t, &with_counts(ENTRY_FIELDS)).unwrap();
    let o = compact_opts(Options {
        compact: compact("{app} {count}/{total} {marker}"),
        ..Options::default()
    });
    assert_eq!(text(&d, &o).as_deref(), Some("app1 1/3 ●"));
    // A progress percentage only for working entries.
    let mut w = r("working", 2, 0);
    w["progress"] = 40.into();
    let o = compact_opts(Options {
        compact: compact("{progress_pct}"),
        ..Options::default()
    });
    assert_eq!(text(&doc(vec![w]), &o).as_deref(), Some("40%"));
}

#[test]
fn age_field() {
    assert_eq!(age(NOW, NOW), "0s");
    assert_eq!(age(100_000, 55_000), "45s");
    assert_eq!(age(NOW, NOW - 180_000), "3m");
    assert_eq!(age(NOW, NOW - 7_300_000), "2h");
    assert_eq!(age(NOW, NOW - 4 * 86_400_000), "4d");
    assert_eq!(age(0, 5), "");
    assert_eq!(age(5, 0), "");
    assert_eq!(age(5, 10), "");
    let o = opts("{app} {age}");
    assert_eq!(
        text(&doc(vec![r("blocked", 1, 95)]), &o).unwrap(),
        "[c=orange]● 1[/c]\\n[c=grey]✗ 0[/c]\\n[c=grey]✓ 0[/c]\\n[c=grey]» 0[/c]|app1 1m"
    );
    assert_eq!(
        render(&doc(vec![r("blocked", 1, 95)]), &o, 0).unwrap(),
        "[c=orange]● 1[/c]\\n[c=grey]✗ 0[/c]\\n[c=grey]✓ 0[/c]\\n[c=grey]» 0[/c]|app1 "
    );
}

#[test]
fn fields_are_truncated_on_a_character_boundary() {
    let o = Options {
        entry: Template::parse("{app:5} {{app:1}} {title:64}", ENTRY_FIELDS).unwrap(),
        ..Options::default()
    };
    let d = doc(vec![r("blocked", 1, 0)]);
    let mut e = entries(&d);
    e[0].app = Some("héllo wörld".into());
    let c = counts(&e);
    assert_eq!(
        o.entry.render(&entry_ctx(&e[0], &c, NOW)),
        "héll… {app:1} pane 1"
    );
    e[0].app = Some("héllo".into());
    assert_eq!(
        o.entry.render(&entry_ctx(&e[0], &c, NOW)),
        "héllo {app:1} pane 1"
    );
    e[0].app = Some("日本語テキスト".into());
    assert_eq!(
        o.entry.render(&entry_ctx(&e[0], &c, NOW)),
        "日本語テ… {app:1} pane 1"
    );
    // The default template truncates the app to 14 characters.
    let mut long = r("blocked", 1, 0);
    long["app"] = "a".repeat(20).into();
    assert!(
        text(&doc(vec![long]), &Options::default())
            .unwrap()
            .contains("[b]aaaaaaaaaaaaa…[/b]")
    );
}

#[test]
fn values_cannot_inject_markup_segments_or_controls() {
    let mut h = r("blocked", 1, 0);
    h["app"] = "[c=red]evil[/c]|x".into();
    h["msg"] = "a|b [b]bold[/b]\n\u{1b}[31mred".into();
    let s = text(&doc(vec![h]), &Options::default()).unwrap();
    assert_eq!(s.split('|').count(), 2, "{s}");
    assert_eq!(
        s,
        "[c=orange]● 1[/c]\\n[c=grey]✗ 0[/c]\\n[c=grey]✓ 0[/c]\\n[c=grey]» 0[/c]|[b][[c=red]evil[[/…[/b]\\n[c=orange]needs you[/c]\\na b [[b]bold[[/b] [[31mr…"
    );
    assert!(!s.contains('\u{1b}') && !s.contains('\n'));
    let o = Options {
        entry: Template::parse("{msg}", ENTRY_FIELDS).unwrap(),
        ..Options::default()
    };
    let d = doc(vec![patch(
        r("idle", 1, 0),
        json!({"msg": "  a \t b |c  ", "app": null}),
    )]);
    let counts_idle = "[c=grey]● 0[/c]\\n[c=grey]✗ 0[/c]\\n[c=grey]✓ 0[/c]\\n[c=grey]» 0[/c]|";
    assert_eq!(text(&d, &o).unwrap(), format!("{counts_idle}a b c"));
    let d = doc(vec![patch(
        r("idle", 1, 0),
        json!({"msg": "[b]\u{7f}x\u{85}", "app": null}),
    )]);
    assert_eq!(text(&d, &o).unwrap(), format!("{counts_idle}[[b] x"));
}

#[test]
fn a_segment_is_capped_by_dropping_trailing_lines() {
    let o = Options {
        entry: Template::parse(
            "{msg:150}\\n{msg:150}\\n{msg:150}\\n{msg:150}",
            ENTRY_FIELDS,
        )
        .unwrap(),
        ..Options::default()
    };
    let d = doc(vec![patch(
        r("working", 1, 0),
        json!({"msg": "x".repeat(150)}),
    )]);
    let e = entries(&d);
    let ctx = entry_ctx(&e[0], &counts(&e), NOW);
    assert_eq!(o.entry.render(&ctx).split("\\n").count(), 4);
    assert_eq!(cap(o.entry.render(&ctx)).chars().count(), 2 * 150 + 2);
    let capped = cap(o.entry.render(&ctx));
    assert_eq!(capped.split("\\n").count(), 2, "{capped}");
    assert!(capped.chars().count() <= 400);
    // The first line always stays, however long.
    assert_eq!(cap("a".repeat(500)), "a".repeat(500));
    assert_eq!(cap("a\\nb\\nc".to_owned()), "a\\nb\\nc");
}

#[test]
fn empty_placeholders_drop_their_line() {
    let o = opts("{msg}\\n[c=grey]fixed[/c]\\n{progress_pct}\\n{tasks}\\nend");
    assert_eq!(
        text(&doc(vec![r("blocked", 1, 0)]), &o).unwrap(),
        "[c=orange]● 1[/c]\\n[c=grey]✗ 0[/c]\\n[c=grey]✓ 0[/c]\\n[c=grey]» 0[/c]|[c=grey]fixed[/c]\\nend"
    );
    // Markup around an empty placeholder does not keep the line either.
    let o = opts("[b]{msg}[/b]\\nend");
    assert_eq!(
        text(&doc(vec![r("blocked", 1, 0)]), &o).unwrap(),
        "[c=orange]● 1[/c]\\n[c=grey]✗ 0[/c]\\n[c=grey]✓ 0[/c]\\n[c=grey]» 0[/c]|end"
    );
    let o = opts("{msg|app}\\nend");
    assert_eq!(
        text(&doc(vec![r("blocked", 1, 0)]), &o).unwrap(),
        "[c=orange]● 1[/c]\\n[c=grey]✗ 0[/c]\\n[c=grey]✓ 0[/c]\\n[c=grey]» 0[/c]|app1\\nend"
    );
}

#[test]
fn the_document_maps_to_entries() {
    let d = sessions(vec![
        (
            "work",
            vec![json!({
                "source": "program_status",
                "tab": {"id": 3, "name": "build", "future": 1},
                "pane": {"id": 7, "title": "  pane seven  "},
                "state": "blocked",
                "kind": "question",
                "app": "claude",
                "title": "Claude Code",
                "msg": "which file?",
                "progress": 90,
                "since_ms": 5,
                "seen": false,
                "muted": false,
                "on_deck": true,
                "local": {"tasks": 2, "layout": {"focused": true, "floating": false}},
                "extra": "ignored"
            })],
        ),
        (
            "peer",
            vec![
                json!({"pane": {"id": 9, "title": "p"}, "tab": {"id": 0}, "state": "working", "on_deck": true}),
            ],
        ),
    ]);
    let e = entries(&d);
    assert_eq!(e.len(), 2);
    assert_eq!(
        e[0],
        Entry {
            tab: 3,
            pane: 7,
            app: Some("claude".into()),
            title: "  pane seven  ".into(),
            state: State::Blocked,
            kind: Some(Kind::Question),
            msg: Some("which file?".into()),
            progress: Some(90),
            children: 2,
            updated_ms: 5,
            session: "work".into(),
            tab_name: "build".into(),
        }
    );
    assert_eq!(
        (
            e[1].children,
            e[1].session.as_str(),
            e[1].tab_name.as_str(),
            e[1].app.as_deref()
        ),
        (0, "peer", "", None)
    );
    let ctx = entry_ctx(&e[0], &counts(&e), NOW);
    let get = |k: &str| ctx[k].as_str();
    assert_eq!(
        (
            get("tab_name"),
            get("session"),
            get("kind"),
            get("state"),
            get("state_label")
        ),
        ("build", "work", "question", "blocked", "has a question")
    );
    assert_eq!(
        (
            get("tasks"),
            get("progress_pct"),
            get("age"),
            get("tab"),
            get("pane"),
            get("count"),
            get("total")
        ),
        ("2 tasks", "", "20833d", "3", "7", "1", "2")
    );
    assert_eq!(
        get("title"),
        "  pane seven  ",
        "the pane title, not the program's"
    );
    assert_eq!(get("msg"), "which file?");
    // One task, kinds, an unknown kind and an unknown state.
    let mut one = r("working", 1, 0);
    one["local"] = json!({"tasks": 1});
    let e = entries(&doc(vec![one]));
    assert_eq!(entry_ctx(&e[0], &counts(&e), NOW)["tasks"], "1 task");
    for (k, label) in [
        ("permission", "needs approval"),
        ("auth", "needs login"),
        ("bogus", "needs you"),
    ] {
        let mut b = r("blocked", 1, 0);
        b["kind"] = k.into();
        let e = entries(&doc(vec![b]));
        let ctx = entry_ctx(&e[0], &counts(&e), NOW);
        assert_eq!(
            ctx["kind"].as_str(),
            if k == "bogus" { "" } else { k },
            "an unknown kind is no kind"
        );
        assert_eq!(
            (
                ctx["state_label"].as_str(),
                ctx["color"].as_str(),
                ctx["marker"].as_str()
            ),
            (label, "orange", "●")
        );
    }
    let u = entries(&doc(vec![r("bogus", 1, 0)]));
    assert_eq!(
        (
            u[0].state,
            state_name(u[0].state),
            state_label(u[0].state, None),
            colour(u[0].state),
            marker(u[0].state)
        ),
        (State::Idle, "idle", "", "grey", "»")
    );
    assert_eq!(
        (state_label(State::Error, None), marker(State::Error)),
        ("failed", "✗")
    );
    assert_eq!(
        (state_label(State::Done, None), marker(State::Done)),
        ("done", "✓")
    );
}

#[test]
fn an_unknown_version_and_missing_optionals_parse() {
    let d: Document = serde_json::from_value(json!({"version": 7, "sessions": [{"name": "s", "panes": [{"pane": {"id": 1}, "on_deck": true}]}]})).unwrap();
    assert_eq!(
        (d.version, d.truncated, d.attention_enabled, d.generated_ms),
        (7, false, true, 0)
    );
    let e = entries(&d);
    assert_eq!(
        (
            e[0].state,
            e[0].session.as_str(),
            e[0].updated_ms,
            e[0].app.as_deref()
        ),
        (State::Idle, "s", 0, None)
    );
    assert!(
        serde_json::from_value::<Document>(json!({"version": 2, "attention_enabled": false}))
            .unwrap()
            .sessions
            .is_empty()
    );
}

#[test]
fn state_tables_match_the_plugin() {
    let table = [
        (State::Idle, "grey", "»", "idle", ""),
        (State::Working, "#55ccff", "»", "working", "working"),
        (State::Done, "green", "✓", "done", "done"),
        (State::Error, "red", "✗", "error", "failed"),
        (State::Blocked, "orange", "●", "blocked", "needs you"),
    ];
    for (s, col, m, name, label) in table {
        assert_eq!(
            (colour(s), marker(s), state_name(s), state_label(s, None)),
            (col, m, name, label)
        );
    }
    let c = Counts {
        needs_you: 1,
        failed: 2,
        done: 3,
        working: 4,
    };
    assert_eq!(
        [
            count_of(&c, State::Blocked),
            count_of(&c, State::Error),
            count_of(&c, State::Done),
            count_of(&c, State::Working),
            count_of(&c, State::Idle)
        ],
        [1, 2, 3, 4, 4]
    );
    let ctx = counts_ctx(&c);
    let get = |k: &str| ctx[k].as_str();
    assert_eq!(
        (
            get("total"),
            get("needs_you"),
            get("needs_you_color"),
            get("failed"),
            get("failed_color"),
            get("working_color")
        ),
        ("10", "1", "orange", "2", "red", "#55ccff")
    );
    let ctx = counts_ctx(&Counts::default());
    assert_eq!(
        (ctx["done_color"].as_str(), ctx["total"].as_str()),
        ("grey", "0")
    );
}

#[test]
fn default_strings_and_templates() {
    let o = Options::default();
    assert!(o.idle.is_empty());
    assert_eq!(o.mode, Mode::Strip);
    assert_eq!(o.slot, None);
    assert_eq!(DEFAULT_IDLE, "");
    assert!(DEFAULT_IDLE_STRIP.starts_with("[c=#66cc88][b]All clear[/b][/c]\\n[c=grey]no failures[/c]\\n[c=grey]nothing running[/c]|[c=grey]waiting for agents…[/c]|"));
}

#[test]
fn options_come_from_the_config() {
    let c = Config::default();
    assert_eq!(Options::from_config(&c), Options::default());
    let mut c = Config {
        zjsb_mode: "compact".into(),
        zjsb_slot: "key2_3".into(),
        zjsb_blank_when_idle: true,
        zjsb_entry_template: "{marker} {app}".into(),
        zjsb_compact_template: "{needs_you}/{total} {state}".into(),
        zjsb_counts_template: "N{needs_you}".into(),
        zjsb_more_template: "{hidden}".into(),
        zjsb_idle_template: "quiet {total}".into(),
        ..Config::default()
    };
    let o = Options::from_config(&c);
    assert_eq!(
        (o.mode, o.slot.as_deref(), o.blank_when_idle),
        (Mode::Compact, Some("key2_3"), true)
    );
    let d = doc(vec![r("blocked", 1, 0)]);
    assert_eq!(text(&d, &o).as_deref(), Some("1/1 blocked"));
    let mut strip = o.clone();
    strip.mode = Mode::Strip;
    assert_eq!(text(&d, &strip).as_deref(), Some("N1|● app1"));
    let mut many = vec![r("blocked", 1, 0), r("blocked", 2, 0)];
    many.extend((3..=6).map(|p| r("done", p, 0)));
    assert!(text(&doc(many), &strip).unwrap().ends_with("|4"));
    assert_eq!(text(&doc(vec![]), &strip).as_deref(), Some("quiet 0"));
    // A template that does not parse keeps the default for that slot only.
    c.zjsb_entry_template = "{bogus}".into();
    c.zjsb_counts_template = "N{needs_you}|".into();
    let o = Options::from_config(&c);
    assert_eq!(o.entry, Options::default().entry);
    assert_eq!(o.counts, Options::default().counts);
    c.zjsb_mode = "strip".into();
    c.zjsb_slot = "dial9".into();
    let o = Options::from_config(&c);
    let d = Template::parse("{needs_you}/{total} {state}", &with_counts(ENTRY_FIELDS)).unwrap();
    assert_eq!((o.mode, o.slot, o.compact), (Mode::Strip, None, d));
    // Each template accepts only its own fields.
    let mut c = Config {
        zjsb_idle_template: "{app}".into(),
        ..Config::default()
    };
    assert_eq!(Options::from_config(&c).idle, Options::default().idle);
    // Valid ones are kept, whatever the other keys hold.
    c.zjsb_idle_template = "quiet".into();
    c.zjsb_counts_template = "{hidden}".into();
    c.zjsb_more_template = "{app}".into();
    c.zjsb_compact_template = "{hidden}".into();
    let o = Options::from_config(&c);
    assert_ne!(o.idle, Options::default().idle);
    assert_eq!(
        (o.counts, o.more, o.compact),
        (
            Options::default().counts,
            Options::default().more,
            Options::default().compact
        )
    );
    c.zjsb_entry_template = "{tab_name}".into();
    assert_ne!(Options::from_config(&c).entry, Options::default().entry);
    c.zjsb_more_template = "{hidden} {needs_you}".into();
    assert_ne!(Options::from_config(&c).more, Options::default().more);
}

#[test]
fn the_examples_apply_their_keys_to_the_defaults() {
    let all = examples::load().unwrap();
    assert_eq!(all.len(), 27);
    let by = |name: &str| all.iter().find(|e| e.name == name).cloned().unwrap();
    let e = by("default-mixed");
    assert!(e.overrides.is_empty());
    assert_eq!(e.config().unwrap().zjsb_mode, "strip");
    assert_eq!(
        by("default-mixed").document().unwrap().sessions[0]
            .panes
            .len(),
        5
    );
    let e = by("compact-custom");
    let c = e.config().unwrap();
    let o = Options::from_config(&c);
    assert_eq!((o.mode, o.slot.as_deref()), (Mode::Compact, Some("dial1")));
    assert_eq!(
        c.zjsb_compact_template,
        "{marker} {count} [c=grey]{age}[/c]\n[b]{app|title:10}[/b]\n{state_label}"
    );
    let want =
        std::fs::read_to_string(examples::dir().join("expected/compact-custom.txt")).unwrap();
    assert_eq!(e.render().unwrap().as_deref(), Some(want.as_str()));
    let ex = |overrides: Vec<(&str, toml::Value)>| examples::Example {
        name: "x".into(),
        scenario: "mixed".into(),
        overrides: overrides
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    };
    let err = |e: examples::Example| e.config().unwrap_err();
    assert_eq!(
        err(ex(vec![("nope", 1.into())])),
        "x: nope is not a renderer key"
    );
    assert!(
        err(ex(vec![("zjsb_slot", 1.into())])).contains("zjsb_slot = 1 is not a non-empty string")
    );
    assert!(err(ex(vec![("zjsb_blank_when_idle", "yes".into())])).contains("is not a boolean"));
    assert!(err(ex(vec![("zjsb_min_interval_ms", 0.into())])).contains("is not a number above 0"));
    let e = ex(vec![
        ("zjsb_min_interval_ms", 250.into()),
        ("zjsb_blank_when_idle", true.into()),
        ("zjsb_idle_template", "q".into()),
    ]);
    let c = e.config().unwrap();
    assert_eq!(
        (
            c.zjsb_min_interval_ms,
            c.zjsb_blank_when_idle,
            c.zjsb_idle_template.as_str()
        ),
        (250, true, "q")
    );
    assert_eq!(
        e.overrides_toml(),
        [
            "zjsb_blank_when_idle = true",
            "zjsb_idle_template = \"q\"",
            "zjsb_min_interval_ms = 250"
        ]
    );
    // Mode and slot lead; a template is one line with `\\n`, quotes and backslashes escaped.
    let e = ex(vec![
        ("zjsb_compact_template", "{a}\n[b]\"q\"[/b]".into()),
        ("zjsb_mode", "compact".into()),
    ]);
    assert_eq!(
        e.overrides_toml(),
        [
            "zjsb_mode = \"compact\"",
            "zjsb_compact_template = \"{a}\\n[b]\\\"q\\\"[/b]\""
        ]
    );
    // The line is TOML for the same value.
    let back: toml::Value = toml::from_str(&e.overrides_toml()[1]).unwrap();
    assert_eq!(
        back["zjsb_compact_template"].as_str(),
        Some("{a}\n[b]\"q\"[/b]")
    );
}

#[test]
fn backslashes_in_values_do_not_become_escapes() {
    use super::template::sanitize;
    assert_eq!(sanitize("C:\\", 64), "C:\\\\");
    assert_eq!(super::cap("a\\\\n b\\nc".to_owned()), "a\\\\n b\\nc");
}

#[test]
fn unsynced_documents_keep_the_deck() {
    use crate::config::Config;
    let doc = Document {
        synced: false,
        ..Default::default()
    };
    let mut w = crate::writer::Writer::new(&Config::default());
    w.stage(&doc, 0);
    assert!(w.deadline().is_none());
}
