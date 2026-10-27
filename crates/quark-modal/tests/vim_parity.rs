//! Common vim commands, through the core, against what vim itself does.
//!
//! One table, because the point is coverage: a modal editor where `f`, `J`
//! or `Vd` silently does something else is one people stop trusting, and
//! each row is a command somebody types without thinking. Every expectation
//! is what vim 9 does with default settings — except that `cc` keeps the
//! line's indentation, as vim does with `autoindent`, and a file's final line
//! break leaves an empty last line for `G` to land on, which is the rope's
//! model rather than vim's.

mod common;
use common::*;

fn keys_of(s: &str) -> Vec<Key> {
    let mut out = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        let named = [
            ("<Esc>", "Escape"),
            ("<CR>", "Enter"),
            ("<BS>", "Backspace"),
            ("<Tab>", "Tab"),
            ("<Up>", "ArrowUp"),
            ("<Down>", "ArrowDown"),
            ("<Left>", "ArrowLeft"),
            ("<Right>", "ArrowRight"),
            ("<Home>", "Home"),
            ("<End>", "End"),
        ];
        if let Some((tok, name)) = named.iter().find(|(t, _)| rest.starts_with(t)) {
            out.push(Key::named(name));
            rest = &rest[tok.len()..];
            continue;
        }
        if rest.starts_with("<C-") {
            let c = rest[3..].chars().next().unwrap();
            out.push(Key::ctrl(c));
            rest = &rest[3 + c.len_utf8() + 1..];
            continue;
        }
        let c = rest.chars().next().unwrap();
        rest = &rest[c.len_utf8()..];
        let mut k = Key::char(c);
        k.shift = c.is_uppercase();
        out.push(k);
    }
    out
}

fn run(text: &str, keys: &str) -> (String, usize, usize, String) {
    let mut ws = Workspace::new();
    // As a paste puts it: typed, its lines would be indented and its brackets
    // closed, and the buffer would not be the one vim is given.
    ws.apply(Command::Key { key: Key::char('i') });
    ws.paste_text(text);
    ws.apply(Command::Key { key: Key::named("Escape") });
    for k in keys_of("gg0") {
        ws.apply(Command::Key { key: k });
    }
    let mut mode = "Normal".to_string();
    for k in keys_of(keys) {
        for event in ws.apply(Command::Key { key: k }) {
            if let Event::VimMode { mode: m, .. } = event {
                mode = format!("{m:?}");
            }
        }
    }
    let buf = ws.editor().buffer();
    let at = buf.position(buf.cursor.offset);
    (buf.contents(), at.line, at.column, mode)
}

#[test]
fn common_commands_do_what_vim_does() {
    let t = "one two three\nfour five six\nseven eight\n";
    // What it is, the text, the keys, the text vim leaves ("" to not check),
    // and where vim leaves the cursor ((99, 99) to not check).
    let cases: &[(&str, &str, &str, &str, (usize, usize))] = &[
        ("w", t, "w", "", (0, 4)),
        ("b", t, "wwb", "", (0, 4)),
        ("e", t, "e", "", (0, 2)),
        ("$", t, "$", "", (0, 12)),
        ("^ after $", "  ab\n", "$^", "", (0, 2)),
        ("G", t, "G", "", (3, 0)),
        ("2G", t, "2G", "", (1, 0)),
        ("fx", t, "ft", "", (0, 4)),
        ("tx", t, "tt", "", (0, 3)),
        (";", t, "fe;", "", (0, 11)),
        ("%", "(a(b))\n", "%", "", (0, 5)),
        ("}", "a\nb\n\nc\n", "}", "", (2, 0)),
        ("dd", t, "dd", "four five six\nseven eight\n", (0, 0)),
        ("dw", t, "dw", "two three\nfour five six\nseven eight\n", (0, 0)),
        ("D", t, "wD", "one \nfour five six\nseven eight\n", (99, 99)),
        ("C", t, "wCX<Esc>", "one X\nfour five six\nseven eight\n", (99, 99)),
        ("x", t, "x", "ne two three\nfour five six\nseven eight\n", (0, 0)),
        ("X", t, "llX", "oe two three\nfour five six\nseven eight\n", (99, 99)),
        ("s", t, "sZ<Esc>", "Zne two three\nfour five six\nseven eight\n", (99, 99)),
        ("S", t, "SZ<Esc>", "Z\nfour five six\nseven eight\n", (99, 99)),
        ("r", t, "rZ", "Zne two three\nfour five six\nseven eight\n", (0, 0)),
        ("~", t, "~", "One two three\nfour five six\nseven eight\n", (0, 1)),
        ("J", t, "J", "one two three four five six\nseven eight\n", (99, 99)),
        ("yyp", t, "yyp", "one two three\none two three\nfour five six\nseven eight\n", (1, 0)),
        ("yyP", t, "yyP", "one two three\none two three\nfour five six\nseven eight\n", (0, 0)),
        ("Y p", t, "Yjp", "one two three\nfour five six\none two three\nseven eight\n", (99, 99)),
        ("yw P", t, "ywP", "one one two three\nfour five six\nseven eight\n", (99, 99)),
        ("cw", t, "cwZ<Esc>", "Z two three\nfour five six\nseven eight\n", (99, 99)),
        ("ciw", t, "wciwZ<Esc>", "one Z three\nfour five six\nseven eight\n", (99, 99)),
        ("di\" inside", "say \"hi there\" ok\n", "fhdi\"", "say \"\" ok\n", (99, 99)),
        ("da( inside", "f(a, b) x\n", "fada(", "f x\n", (99, 99)),
        ("de", t, "de", " two three\nfour five six\nseven eight\n", (99, 99)),
        ("ce", t, "ceZ<Esc>", "Z two three\nfour five six\nseven eight\n", (99, 99)),
        ("dt", t, "dtt", "two three\nfour five six\nseven eight\n", (99, 99)),
        ("df", t, "dfo", " three\nfour five six\nseven eight\n", (99, 99)),
        ("dF", t, "$dFt", "one two e\nfour five six\nseven eight\n", (99, 99)),
        (",", t, "fe;,", "", (0, 2)),
        ("d}", "a\nb\n\nc\n", "d}", "\nc\n", (99, 99)),
        ("{", "a\n\nb\nc\n", "G{", "", (1, 0)),
        ("d%", "x(a(b))y\n", "fxd%", "y\n", (99, 99)),
        ("5G", &"x\n".repeat(9), "5G", "", (4, 0)),
        ("3gg", &"x\n".repeat(9), "3gg", "", (2, 0)),
        ("dG", t, "jdG", "one two three\n", (99, 99)),
        ("3J", t, "3J", "one two three four five six seven eight\n", (99, 99)),
        ("VjJ", t, "VjJ", "one two three four five six\nseven eight\n", (99, 99)),
        ("3~", t, "3~", "ONE two three\nfour five six\nseven eight\n", (0, 3)),
        ("g~w", t, "g~w", "ONE two three\nfour five six\nseven eight\n", (99, 99)),
        ("gUU", t, "gUU", "ONE TWO THREE\nfour five six\nseven eight\n", (99, 99)),
        ("Vu", "ABC\nDEF\n", "Vju", "abc\ndef\n", (99, 99)),
        ("3rx", t, "3rx", "xxx two three\nfour five six\nseven eight\n", (0, 2)),
        ("vlr", t, "vlrq", "qqe two three\nfour five six\nseven eight\n", (99, 99)),
        ("Vc", t, "jVcZ<Esc>", "one two three\nZ\nseven eight\n", (99, 99)),
        ("cc indent", "  ab\nx\n", "ccZ<Esc>", "  Z\nx\n", (99, 99)),
        ("insert <C-u>", t, "A<C-u>Z<Esc>", "Z\nfour five six\nseven eight\n", (99, 99)),
        ("insert <C-h>", t, "A<C-h><Esc>", "one two thre\nfour five six\nseven eight\n", (99, 99)),
        ("l at end", "ab\n", "lll", "", (0, 1)),
        ("j shorter", "abcdef\nab\n", "$j", "", (1, 1)),
        ("x at end", "abc\n", "$x", "ab\n", (0, 1)),
        (">j", t, ">j", "    one two three\n    four five six\nseven eight\n", (0, 4)),
        ("3>>", t, "3>>", "    one two three\n    four five six\n    seven eight\n", (99, 99)),
        // The arrows are hjkl outside insert mode too.
        ("<Down> <Right>", t, "<Down><Right>", "", (1, 1)),
        ("<End>", t, "<End>", "", (0, 12)),
        ("V<Down>d", t, "V<Down>d", "seven eight\n", (99, 99)),
        ("V<Down><Down>y P", t, "V<Down><Down>yGP", "one two three\nfour five six\nseven eight\none two three\nfour five six\nseven eight\n", (99, 99)),
        ("v<Right>d", t, "v<Right>d", "e two three\nfour five six\nseven eight\n", (99, 99)),
        ("d<Down>", t, "d<Down>", "seven eight\n", (99, 99)),
        ("u", t, "ddu", t, (99, 99)),
        ("<C-r>", t, "ddu<C-r>", "four five six\nseven eight\n", (99, 99)),
        (".", t, "dd.", "seven eight\n", (99, 99)),
        (">>", t, ">>", "    one two three\nfour five six\nseven eight\n", (99, 99)),
        ("<<", "    ab\n", "<<", "ab\n", (99, 99)),
        ("V> ", t, "Vj>", "    one two three\n    four five six\nseven eight\n", (99, 99)),
        ("Vd", t, "Vd", "four five six\nseven eight\n", (99, 99)),
        ("Vjd", t, "Vjd", "seven eight\n", (99, 99)),
        ("Vy p", t, "Vyjp", "one two three\nfour five six\none two three\nseven eight\n", (99, 99)),
        ("vd", t, "vlld", " two three\nfour five six\nseven eight\n", (99, 99)),
        ("v~", t, "vll~", "ONE two three\nfour five six\nseven eight\n", (99, 99)),
        ("vU", t, "vllU", "ONE two three\nfour five six\nseven eight\n", (99, 99)),
        ("gUiw", t, "gUiw", "ONE two three\nfour five six\nseven eight\n", (99, 99)),
        ("guu", "ABC\n", "guu", "abc\n", (99, 99)),
        ("<C-v>jd", "abc\ndef\n", "<C-v>jd", "bc\nef\n", (99, 99)),
        ("o", t, "oZ<Esc>", "one two three\nZ\nfour five six\nseven eight\n", (99, 99)),
        ("O", t, "OZ<Esc>", "Z\none two three\nfour five six\nseven eight\n", (99, 99)),
        ("A", t, "AZ<Esc>", "one two threeZ\nfour five six\nseven eight\n", (99, 99)),
        ("I", "  ab\n", "IZ<Esc>", "  Zab\n", (99, 99)),
        ("a", t, "aZ<Esc>", "oZne two three\nfour five six\nseven eight\n", (99, 99)),
        ("insert <C-w>", t, "A<C-w><Esc>", "one two \nfour five six\nseven eight\n", (99, 99)),
        ("insert <BS>", t, "A<BS><Esc>", "one two thre\nfour five six\nseven eight\n", (99, 99)),
        ("/ search", t, "/five<CR>", "", (1, 5)),
        ("*", "foo bar foo\n", "*", "", (0, 8)),
        ("qa macro", "a\nb\nc\n", "qaA!<Esc>jq@a", "a!\nb!\nc\n", (99, 99)),
        ("<C-d>", &"x\n".repeat(100), "<C-d>", "", (99, 99)),
        ("ma 'a", t, "majj'a", "", (0, 0)),
        ("gv", t, "Vj<Esc>gvd", "seven eight\n", (99, 99)),
        ("xp", "ab\n", "xp", "ba\n", (99, 99)),
        ("ddp", "a\nb\n", "ddp", "b\na\n", (99, 99)),
        ("3x", t, "3x", " two three\nfour five six\nseven eight\n", (99, 99)),
        ("d$", t, "wd$", "one \nfour five six\nseven eight\n", (99, 99)),
        ("cc", t, "ccZ<Esc>", "Z\nfour five six\nseven eight\n", (99, 99)),
        ("yiw P", t, "yiwP", "oneone two three\nfour five six\nseven eight\n", (99, 99)),
    ];
    let mut differ = Vec::new();
    for (name, text, keys, want, at) in cases {
        let (got, line, col, mode) = run(text, keys);
        let text_ok = want.is_empty() || got == *want;
        let at_ok = *at == (99, 99) || (line, col) == *at;
        if !(text_ok && at_ok && mode == "Normal") {
            differ.push(format!(
                "{name} ({keys}): got {got:?} at ({line},{col}) in {mode}; vim gives {want:?} at {at:?}"
            ));
        }
    }
    assert!(differ.is_empty(), "{} of {} differ from vim:\n{}", differ.len(), cases.len(), differ.join("\n"));
}
