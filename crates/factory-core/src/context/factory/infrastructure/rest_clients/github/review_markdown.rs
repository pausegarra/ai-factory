use std::borrow::Cow;

const PARAGRAPH_TARGET: usize = 320;

pub(super) fn format_review(body: &str) -> Cow<'_, str> {
    let prose = body.trim();
    // Restrict the fallback to prose on a single line. Existing Markdown
    // blocks and indentation already carry structure.
    if prose.len() < PARAGRAPH_TARGET
        || body.contains(['\n', '\r'])
        || body.starts_with(' ')
        || body.starts_with('\t')
        || !prose.starts_with(char::is_alphabetic)
    {
        return Cow::Borrowed(body);
    }
    let Some(sentences) = sentences(prose) else {
        return Cow::Borrowed(body);
    };

    let mut markdown = String::with_capacity(body.len());
    let mut paragraph_start = 0;
    let mut findings = false;
    for sentence in sentences {
        if let Some((title, text, is_findings)) = section(sentence) {
            if !markdown.is_empty() {
                markdown.push_str("\n\n");
            }
            markdown.push_str("## ");
            markdown.push_str(title);
            markdown.push_str("\n\n");
            paragraph_start = markdown.len();
            findings = is_findings;
            if findings {
                markdown.push_str("- ");
            }
            markdown.push_str(text);
        } else if findings && starts_file_reference(sentence) {
            markdown.push_str("\n- ");
            markdown.push_str(sentence);
        } else {
            if !findings && markdown.len() - paragraph_start >= PARAGRAPH_TARGET {
                markdown.push_str("\n\n");
                paragraph_start = markdown.len();
            } else if !markdown.is_empty() {
                markdown.push(' ');
            }
            markdown.push_str(sentence);
        }
    }
    if markdown.contains('\n') {
        Cow::Owned(markdown)
    } else {
        Cow::Borrowed(body)
    }
}

fn section(sentence: &str) -> Option<(&str, &str, bool)> {
    let (title, text) = sentence.split_once(": ")?;
    let findings = match title.to_lowercase().as_str() {
        "findings"
        | "blocking findings"
        | "non-blocking findings"
        | "hallazgos"
        | "hallazgos bloqueantes"
        | "hallazgos no bloqueantes" => true,
        "decision" | "decisión" | "summary" | "resumen" | "changes" | "cambios"
        | "verification" | "verificación" | "verificaciones" | "limitations" | "limitaciones"
        | "no bloqueantes" => false,
        _ => return None,
    };
    Some((title, text, findings))
}

fn starts_file_reference(sentence: &str) -> bool {
    let Some((lead, reference)) = sentence.split_once(" `") else {
        return false;
    };
    if !lead.eq_ignore_ascii_case("in") && !lead.eq_ignore_ascii_case("en") {
        return false;
    }
    reference
        .split_once('`')
        .and_then(|(reference, _)| reference.rsplit_once(':'))
        .is_some_and(|(path, line)| {
            !path.is_empty() && !line.is_empty() && line.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn sentences(prose: &str) -> Option<Vec<&str>> {
    let mut sentences = Vec::new();
    let mut chars = prose.char_indices().peekable();
    let mut start = 0;
    let mut code_delimiter = 0;
    let mut bracket_depth = 0_usize;
    while let Some((index, ch)) = chars.next() {
        if ch == '`' {
            let mut count = 1;
            while chars.next_if(|&(_, next)| next == '`').is_some() {
                count += 1;
            }
            if code_delimiter == 0 {
                code_delimiter = count;
            } else if code_delimiter == count {
                code_delimiter = 0;
            }
            continue;
        }
        if code_delimiter != 0 {
            continue;
        }
        match ch {
            '\\' => {
                chars.next();
            }
            '[' | '(' => bracket_depth += 1,
            ']' | ')' => bracket_depth = bracket_depth.saturating_sub(1),
            '<' if bracket_depth == 0
                && chars.peek().is_some_and(|&(_, next)| {
                    next.is_ascii_alphabetic() || matches!(next, '/' | '!' | '?')
                }) =>
            {
                // Preserve raw HTML and autolinks outside inline code.
                return None;
            }
            '.' | '!' | '?' if bracket_depth == 0 => {
                let rest = &prose[index + 1..];
                let next = rest.trim_start();
                if next.len() < rest.len() && next.starts_with(char::is_uppercase) {
                    sentences.push(&prose[start..=index]);
                    start = prose.len() - next.len();
                }
            }
            _ => {}
        }
    }
    sentences.push(&prose[start..]);
    Some(sentences)
}
