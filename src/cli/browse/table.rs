//! Markdown tables drawn as aligned grids with box-drawing borders. Display
//! only: grid rows are built from byte ranges of the source lines, so
//! styling, links and search hits still map back to the unchanged markdown.

use std::iter;
use std::mem;
use std::ops::Range;

use unicode_width::UnicodeWidthStr;

use super::doc::{Row, wrap_line};

/// Narrowest a column may get (or its natural width, if less) before the
/// table is shown as plain markdown instead.
const MIN_COL: usize = 4;

/// Columns are narrowed to their longest word, up to this wide, before
/// any words are broken.
const MAX_WORD: usize = 16;

/// Rows above the first separator are headers when there are at most this
/// many; otherwise only the first row is.
const MAX_HEADER: usize = 2;

/// Part of a table's display row.
#[derive(Debug, Clone)]
pub enum Piece {
    /// Borders and padding, drawn dimmed.
    Border(String),
    /// Cell text: a byte range of the row's source line, bold in header and
    /// full-width rows.
    Text { bytes: Range<usize>, strong: bool },
}

pub fn is_table_line(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

/// A table row with some text.
struct Cells {
    line: usize,
    /// Trimmed cell text, as byte ranges of the line.
    cells: Vec<Range<usize>>,
    /// Only the first cell has text: drawn across the whole table.
    span: bool,
    strong: bool,
}

enum Item {
    /// A horizontal rule (separator row, or around headers and full-width
    /// rows), attributed to this source line.
    Rule(usize),
    Row(Cells),
}

/// Lay out the table on source lines `block` within `width` columns. `None`
/// when it is not a table (no content or separator row) or its columns can't
/// fit, so the markdown is shown as is.
pub fn layout(lines: &[String], block: Range<usize>, width: usize) -> Option<Vec<Row>> {
    // Separator rows parse as `None`; rows without any text are dropped
    let parsed: Vec<(usize, Option<Vec<Range<usize>>>)> = block
        .clone()
        .map(|i| (i, parse(&lines[i])))
        .filter(|(_, cells)| {
            cells
                .as_ref()
                .is_none_or(|c| c.iter().any(|r| !r.is_empty()))
        })
        .collect();
    let ncols = parsed
        .iter()
        .filter_map(|(_, c)| c.as_ref().map(Vec::len))
        .max()?;
    // The converter puts the separator after the rows that have `<th>`
    // cells, which in infoboxes can be anywhere: only trust an early one.
    let first_sep = parsed.iter().position(|(_, c)| c.is_none())?;
    let header = if (1..=MAX_HEADER).contains(&first_sep) && first_sep + 1 < parsed.len() {
        first_sep
    } else {
        1
    };

    let mut items = Vec::new();
    let mut rule = None;
    let mut seen = 0;
    let mut prev_span = false;
    for (line, cells) in parsed {
        let Some(cells) = cells else {
            if seen > 0 {
                rule.get_or_insert(line);
            }
            continue;
        };
        let span = ncols > 1 && cells[1..].iter().all(Range::is_empty);
        // Set the header and full-width rows apart
        if seen > 0 && (span || prev_span || seen == header) {
            rule.get_or_insert(line);
        }
        if let Some(at) = rule.take() {
            items.push(Item::Rule(at));
        }
        let strong = span || seen < header;
        items.push(Item::Row(Cells {
            line,
            cells,
            span,
            strong,
        }));
        seen += 1;
        prev_span = span;
    }

    let mut natural = vec![0; ncols];
    let mut words = vec![MIN_COL; ncols];
    let mut span_width = 0;
    for item in &items {
        let Item::Row(row) = item else { continue };
        let text = &lines[row.line];
        if row.span {
            span_width = span_width.max(text[row.cells[0].clone()].width());
            continue;
        }
        for (j, cell) in row.cells.iter().enumerate() {
            let cell = &text[cell.clone()];
            natural[j] = natural[j].max(cell.width());
            let word = cell.split(' ').map(str::width).max().unwrap_or(0);
            words[j] = words[j].max(word.min(MAX_WORD));
        }
    }
    let mut widths = fit(&natural, &words, width.checked_sub(3 * ncols + 1)?)?;
    // Full-width rows may widen the table, up to the screen width
    let wanted = span_width.min(width.saturating_sub(4));
    widths[ncols - 1] += wanted.saturating_sub(inner_width(&widths));

    // Whether item `i` is a row split into columns; `None` past the edges
    let split = |i: usize| match items.get(i) {
        Some(Item::Row(row)) => Some(!row.span),
        _ => None,
    };
    let last = block.end - 1;
    let mut rows = vec![border(block.start, 0, rule_text(&widths, None, split(0)))];
    for (i, item) in items.iter().enumerate() {
        match item {
            Item::Rule(at) => {
                let text = rule_text(&widths, split(i - 1), split(i + 1));
                rows.push(border(*at, 0, text));
            }
            Item::Row(row) => draw_row(&lines[row.line], row, &widths, &mut rows),
        }
    }
    let bottom = rule_text(&widths, split(items.len() - 1), None);
    rows.push(border(last, lines[last].len(), bottom));
    Some(rows)
}

/// Trimmed cell text of a table line as byte ranges, or `None` for a
/// separator row such as `|---|:--:|`.
fn parse(line: &str) -> Option<Vec<Range<usize>>> {
    let body = line.trim_end();
    if body.contains('-') && body.bytes().all(|c| matches!(c, b'|' | b'-' | b':' | b' ')) {
        return None;
    }
    let b = body.as_bytes();
    let lead = body.len() - body.trim_start().len();
    let mut pipes: Vec<usize> = (lead..b.len())
        .filter(|&i| b[i] == b'|' && (i == 0 || b[i - 1] != b'\\'))
        .collect();
    // The closing pipe is optional
    if pipes.last() != Some(&(b.len() - 1)) {
        pipes.push(b.len());
    }
    Some(
        pipes
            .windows(2)
            .map(|w| trim(body, w[0] + 1..w[1]))
            .collect(),
    )
}

fn trim(s: &str, r: Range<usize>) -> Range<usize> {
    let cell = &s[r.clone()];
    let start = r.start + cell.len() - cell.trim_start().len();
    start..start + cell.trim().len()
}

/// Column widths within `budget`: the natural widths if they fit, else the
/// widest columns narrowed first, down to their longest word (`words`) and
/// then, breaking words, to `MIN_COL`. `None` if even that is too wide.
fn fit(natural: &[usize], words: &[usize], budget: usize) -> Option<Vec<usize>> {
    let mut widths = natural.to_vec();
    let mut excess = widths.iter().sum::<usize>().saturating_sub(budget);
    let min = vec![MIN_COL; widths.len()];
    for floor in [words, &min] {
        while excess > 0 {
            let Some(widest) = (0..widths.len())
                .filter(|&j| widths[j] > floor[j])
                .max_by_key(|&j| widths[j])
            else {
                break;
            };
            widths[widest] -= 1;
            excess -= 1;
        }
    }
    (excess == 0).then_some(widths)
}

/// Text width between the outer borders.
fn inner_width(widths: &[usize]) -> usize {
    widths.iter().sum::<usize>() + 3 * (widths.len() - 1)
}

/// Display rows for one table row, with each cell word-wrapped in its column.
fn draw_row(text: &str, row: &Cells, widths: &[usize], out: &mut Vec<Row>) {
    let columns: Vec<(usize, Vec<Range<usize>>)> = if row.span {
        let w = inner_width(widths);
        vec![(w, wrap_cell(text, &row.cells[0], w))]
    } else {
        widths
            .iter()
            .enumerate()
            .map(|(j, &w)| {
                let segs = row
                    .cells
                    .get(j)
                    .map_or_else(Vec::new, |c| wrap_cell(text, c, w));
                (w, segs)
            })
            .collect()
    };
    let height = columns
        .iter()
        .map(|(_, segs)| segs.len())
        .max()
        .unwrap_or(0);
    for k in 0..height {
        let mut pieces = Vec::new();
        let mut pad = String::from("│ ");
        let (mut start, mut end) = (usize::MAX, 0);
        for (j, (w, segs)) in columns.iter().enumerate() {
            let mut used = 0;
            if let Some(seg) = segs.get(k) {
                pieces.push(Piece::Border(mem::take(&mut pad)));
                pieces.push(Piece::Text {
                    bytes: seg.clone(),
                    strong: row.strong,
                });
                used = text[seg.clone()].width();
                start = start.min(seg.start);
                end = end.max(seg.end);
            }
            pad.extend(iter::repeat_n(' ', w.saturating_sub(used)));
            pad.push_str(if j + 1 < columns.len() {
                " │ "
            } else {
                " │"
            });
        }
        pieces.push(Piece::Border(pad));
        out.push(Row {
            line: row.line,
            start,
            end,
            grid: Some(pieces),
        });
    }
}

/// Word-wrap the cell at `cell` to `width` columns, as byte ranges of `text`.
fn wrap_cell(text: &str, cell: &Range<usize>, width: usize) -> Vec<Range<usize>> {
    let mut segs = Vec::new();
    if !cell.is_empty() {
        let s = &text[cell.clone()];
        wrap_line(s, width.max(1), |a, b| {
            // A break just before a space leaves the space for the next row
            let seg = &s[a..b];
            let a = a + seg.len() - seg.trim_start().len();
            let b = a + seg.trim().len();
            if a < b {
                segs.push(cell.start + a..cell.start + b);
            }
        });
    }
    segs
}

/// A display row of border text only, ordered at byte `at` of `line`.
fn border(line: usize, at: usize, text: String) -> Row {
    Row {
        line,
        start: at,
        end: at,
        grid: Some(vec![Piece::Border(text)]),
    }
}

/// A horizontal border. `above`/`below` tell whether the row on that side is
/// split into columns; `None` is the table's top or bottom edge.
fn rule_text(widths: &[usize], above: Option<bool>, below: Option<bool>) -> String {
    let (left, right) = match (above, below) {
        (None, _) => ('┌', '┐'),
        (_, None) => ('└', '┘'),
        _ => ('├', '┤'),
    };
    let joint = match (above == Some(true), below == Some(true)) {
        (true, true) => '┼',
        (true, false) => '┴',
        (false, true) => '┬',
        (false, false) => '─',
    };
    let mut s = String::from(left);
    for (j, &w) in widths.iter().enumerate() {
        if j > 0 {
            s.push(joint);
        }
        s.extend(iter::repeat_n('─', w + 2));
    }
    s.push(right);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(md: &str) -> Vec<String> {
        md.lines().map(String::from).collect()
    }

    /// A display row as plain text.
    fn text(line: &str, row: &Row) -> String {
        let pieces = row.grid.as_ref().expect("grid row");
        pieces
            .iter()
            .map(|p| match p {
                Piece::Border(s) => s.as_str(),
                Piece::Text { bytes, .. } => &line[bytes.clone()],
            })
            .collect()
    }

    fn grid(md: &str, width: usize) -> Vec<String> {
        let lines = lines(md);
        let rows = layout(&lines, 0..lines.len(), width).expect("laid out");
        rows.iter().map(|r| text(&lines[r.line], r)).collect()
    }

    fn strong(md: &str) -> Vec<bool> {
        let lines = lines(md);
        let rows = layout(&lines, 0..lines.len(), 80).unwrap();
        rows.iter()
            .filter_map(|r| {
                r.grid.as_ref().unwrap().iter().find_map(|p| match p {
                    Piece::Text { strong, .. } => Some(*strong),
                    Piece::Border(_) => None,
                })
            })
            .collect()
    }

    #[test]
    fn parses_cells() {
        let line = "| a | b c |  | d \\| e |";
        let cells: Vec<_> = parse(line).unwrap().into_iter().map(|r| &line[r]).collect();
        assert_eq!(cells, ["a", "b c", "", "d \\| e"]);
        let open = "| x | y";
        let cells: Vec<_> = parse(open).unwrap().into_iter().map(|r| &open[r]).collect();
        assert_eq!(cells, ["x", "y"]);
        assert!(parse("|---|:--:|").is_none());
        assert_eq!(parse("|  |  |").unwrap().len(), 2);
    }

    #[test]
    fn aligns_columns_at_natural_width() {
        assert_eq!(
            grid("| a | bb |\n|---|---|\n| ccc | d |", 80),
            [
                "┌─────┬────┐",
                "│ a   │ bb │",
                "├─────┼────┤",
                "│ ccc │ d  │",
                "└─────┴────┘",
            ]
        );
    }

    #[test]
    fn shrinks_widest_column_and_wraps_cells() {
        let md = "| Name | Value |\n|---|---|\n| Site | Pacific Ocean southwest of San Diego |";
        let g = grid(md, 30);
        assert_eq!(
            g,
            [
                "┌──────┬─────────────────────┐",
                "│ Name │ Value               │",
                "├──────┼─────────────────────┤",
                "│ Site │ Pacific Ocean       │",
                "│      │ southwest of San    │",
                "│      │ Diego               │",
                "└──────┴─────────────────────┘",
            ]
        );
        assert!(g.iter().all(|r| r.width() == 30));
        // A word that exactly fills the column doesn't leave a blank row
        assert_eq!(
            grid("| Source |\n|---|\n| Official projection |", 12)[3..5],
            ["│ Official │", "│ projecti │"]
        );
    }

    #[test]
    fn breaks_words_only_when_needed() {
        // Plain widest-first would make both 11 wide and split the long word
        assert_eq!(fit(&[12, 20], &[12, 4], 22), Some(vec![12, 10]));
        assert_eq!(fit(&[12, 20], &[12, 4], 14), Some(vec![10, 4]));
        assert_eq!(fit(&[12, 20], &[12, 4], 7), None);
        assert_eq!(fit(&[2, 3], &[4, 4], 5), Some(vec![2, 3]));
    }

    #[test]
    fn falls_back_when_columns_cannot_fit() {
        let lines = lines("| a | b | c |\n|---|---|---|\n| one | two | three |");
        // Borders take 10 columns; "three" needs at least MIN_COL of them
        assert!(layout(&lines, 0..3, 19).is_none());
        assert!(layout(&lines, 0..3, 20).is_some());
        // Not a table without a separator row
        assert!(layout(&lines, 0..1, 80).is_none());
    }

    #[test]
    fn collapses_empty_rows_and_spans_lone_cells() {
        let md =
            "| Caption |  |\n| A | 1 |\n|  |  |\n| Section |  |\n| B | 2 |\n|---|---|\n|  |  |";
        assert_eq!(
            grid(md, 80),
            [
                "┌─────────┐",
                "│ Caption │",
                "├───┬─────┤",
                "│ A │ 1   │",
                "├───┴─────┤",
                "│ Section │",
                "├───┬─────┤",
                "│ B │ 2   │",
                "└───┴─────┘",
            ]
        );
        assert_eq!(strong(md), [true, false, true, false]);
    }

    #[test]
    fn draws_separators_as_rules() {
        // Mid-table separator (as in infoboxes); a trailing one is dropped
        let md = "| a | 1 |\n| b | 2 |\n| c | 3 |\n|---|---|\n| d | 4 |\n|---|---|";
        assert_eq!(
            grid(md, 80),
            [
                "┌───┬───┐",
                "│ a │ 1 │",
                "├───┼───┤",
                "│ b │ 2 │",
                "│ c │ 3 │",
                "├───┼───┤",
                "│ d │ 4 │",
                "└───┴───┘",
            ]
        );
    }

    #[test]
    fn marks_header_rows() {
        assert_eq!(strong("| H |\n|---|\n| a |\n| b |"), [true, false, false]);
        // Two header rows above an early separator
        assert_eq!(strong("| H |\n| h |\n|---|\n| a |"), [true, true, false]);
        // A late or trailing separator only marks the first row
        assert_eq!(
            strong("| H |\n| a |\n| b |\n|---|\n| c |"),
            [true, false, false, false]
        );
        assert_eq!(strong("| H |\n| a |\n|---|"), [true, false]);
    }

    #[test]
    fn aligns_wide_characters() {
        let md = "| 日本語 | x |\n|---|---|\n| abc | 東京タワー |";
        let g = grid(md, 80);
        assert_eq!(g[1], "│ 日本語 │ x          │");
        assert_eq!(g[3], "│ abc    │ 東京タワー │");
        let narrow = grid(md, 16);
        assert!(narrow.len() > g.len());
        assert!(narrow.iter().all(|r| r.width() == narrow[0].width()));
        assert!(narrow[0].width() <= 16);
    }

    #[test]
    fn rows_keep_source_positions() {
        let lines = lines("| k | alpha beta gamma |\n|---|---|\n| k2 | v |");
        let rows = layout(&lines, 0..3, 16).unwrap();
        let at: Vec<_> = rows.iter().map(|r| (r.line, r.start)).collect();
        // Sorted by (line, start), with cell text bytes in range
        assert!(at.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(at[0], (0, 0));
        assert_eq!(text(&lines[0], &rows[2]), "│    │ beta    │");
        assert_eq!(&lines[0][rows[2].start..rows[2].end], "beta");
    }
}
