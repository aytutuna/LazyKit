mod common;

use common::{golden, latin, unescape};
use lazy_pdf_syntax::{Lexer, Object};

// The reference engines' writers golden file. What they write needs the font, annotation and editing layers that are
// not in this crate, so what is checked here is the syntax half: every object they write reads back and is written
// again as the same text, and every stream's data is the text the file says.

fn objects(output: &str) -> Vec<(String, Vec<(u32, String)>)> {
    let mut cases: Vec<(String, Vec<(u32, String)>)> = Vec::new();
    for line in output.lines() {
        if let Some(command) = line.strip_prefix("> ") {
            cases.push((command.to_string(), Vec::new()));
        } else {
            let (number, text) = line.split_once(": ").unwrap();
            cases
                .last_mut()
                .unwrap()
                .1
                .push((number.parse().unwrap(), text.to_string()));
        }
    }
    cases
}

fn read(text: &str) -> Object {
    let bytes = latin(text);
    Lexer::new(&bytes, 0).read_object(false).unwrap()
}

#[test]
fn every_case_of_the_input_has_its_output() {
    let commands: Vec<String> = golden("writers/writers-input.txt")
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();
    let cases = objects(&golden("writers/writers-output.txt"));
    assert_eq!(
        cases.iter().map(|c| c.0.clone()).collect::<Vec<_>>(),
        commands
    );
    for (command, objects) in &cases {
        let numbers: Vec<u32> = objects.iter().map(|o| o.0).collect();
        assert_eq!(
            numbers,
            (1..=numbers.len() as u32).collect::<Vec<_>>(),
            "{command}"
        );
    }
}

#[test]
fn what_the_writers_write_is_written_again_as_the_same_text() {
    let mut checked = 0;
    for (command, objects) in objects(&golden("writers/writers-output.txt")) {
        for (number, text) in objects {
            let context = format!("{command}, object {number}");
            if text == "stream font program" || text == "null" {
                continue;
            }
            if let Some(stream) = text.strip_prefix("stream ") {
                let (dict, data) = stream.split_once(" | ").unwrap();
                assert_eq!(read(dict).to_string(), dict, "{context}");
                let bytes = unescape(data);
                assert_eq!(common::escape(&bytes), data, "{context}");
            } else {
                assert_eq!(read(&text).to_string(), text, "{context}");
            }
            checked += 1;
        }
    }
    assert!(checked > 400, "{checked}");
}
