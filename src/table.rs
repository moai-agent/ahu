//! Terminal-width table layout.
//!
//! A listing is read at whatever width the reader's terminal happens to be. A
//! row that wraps turns a table into a wall of text, so this lays one out to
//! fit: the widest, most compressible column gives up width first, then the
//! lower-value ones shrink, then they leave the table. Nothing wraps.
//!
//! Columns marked as neither shrinking nor dropping keep their full width. A
//! truncated task handle is not a handle -- the reader cannot paste it into the
//! next command -- so `ahu tasks` declares that column fixed and the table
//! overflows rather than hand back an identifier that does not resolve.

use std::io::IsTerminal;

use crate::style::{Role, Style};

/// The width assumed when nothing reports one.
pub const DEFAULT_COLUMNS: usize = 80;

/// Narrower than this, no column plan helps; laying out for it would only
/// produce a table of ellipses.
const MIN_COLUMNS: usize = 40;

/// Blank columns between two cells.
const GAP: usize = 2;

/// The width to lay a table out for.
pub fn columns() -> usize {
    resolve(explicit_columns(), tty_columns)
}

/// An explicit `COLUMNS` wins, because a reader who set it means it; otherwise
/// the tty reports its own size, and a redirected or unreadable stdout falls
/// back to the conventional 80. Nothing narrower than [`MIN_COLUMNS`] is
/// honored: below it every column is an ellipsis, which reports nothing.
fn resolve(explicit: Option<usize>, tty: impl FnOnce() -> Option<usize>) -> usize {
    explicit
        .or_else(tty)
        .unwrap_or(DEFAULT_COLUMNS)
        .max(MIN_COLUMNS)
}

fn explicit_columns() -> Option<usize> {
    std::env::var("COLUMNS")
        .ok()?
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
}

fn tty_columns() -> Option<usize> {
    use std::os::unix::io::AsRawFd;
    let stdout = std::io::stdout();
    if !stdout.is_terminal() {
        return None;
    }
    let mut size = std::mem::MaybeUninit::<libc::winsize>::uninit();
    // SAFETY: TIOCGWINSZ writes one `winsize` through the pointer, which owns
    // storage that outlives the call, and the descriptor is open for its
    // duration.
    let read = unsafe { libc::ioctl(stdout.as_raw_fd(), libc::TIOCGWINSZ, size.as_mut_ptr()) };
    if read != 0 {
        return None;
    }
    // SAFETY: the ioctl reported success, so it initialized the value.
    let size = unsafe { size.assume_init() };
    (size.ws_col > 0).then_some(size.ws_col as usize)
}

/// One column's layout policy.
pub struct Column {
    pub header: &'static str,
    /// The narrowest width this column is still worth reading at. Ignored when
    /// the column does not shrink.
    pub min: usize,
    /// Ascending order in which columns give up width, and the order they are
    /// offered it back in. `None` never shrinks: the column is shown at its
    /// full width or not at all.
    pub shrink: Option<u8>,
    /// Ascending order in which columns leave the table. `None` never leaves.
    pub drop: Option<u8>,
}

/// One cell: display-safe text and the role it is painted in.
///
/// The text is escaped by the caller, which is the only place that knows
/// whether a value came out of a record. `Style::paint` neutralizes anything
/// that got through, and an unstyled cell is left exactly as given.
pub struct Cell {
    pub text: String,
    pub role: Option<Role>,
}

impl Cell {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            role: None,
        }
    }

    pub fn painted(role: Role, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            role: Some(role),
        }
    }
}

/// Terminal columns a character occupies, budgeted conservatively: a non-ASCII
/// glyph may be double width. The ellipsis this module appends is the one
/// exception; terminals render it narrow.
fn char_width(c: char) -> usize {
    match c {
        '…' => 1,
        c if c.is_ascii() => 1,
        _ => 2,
    }
}

fn width_of(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

/// Cut `text` to at most `width` columns, marking the cut with an ellipsis.
fn fit(text: &str, width: usize) -> String {
    if width_of(text) <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut used = 0;
    let mut out = String::new();
    for c in text.chars() {
        let next = char_width(c);
        // The ellipsis needs the last column of the budget.
        if used + next > width - 1 {
            break;
        }
        out.push(c);
        used += next;
    }
    out.push('…');
    out
}

/// The width each column is given, or `None` for one that left the table.
///
/// Columns leave the table until what remains fits with every shrinkable
/// column at its floor; whatever the survivors then leave unused goes back to
/// the shrinkable ones, most compressible first. A column shrunk past the
/// point where it still says something is worse than one that is simply gone,
/// which is why dropping is decided against the floor rather than the width a
/// column would like.
fn plan(columns: &[Column], rows: &[Vec<Cell>], width: usize) -> Vec<Option<usize>> {
    let natural: Vec<usize> = columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            rows.iter()
                .filter_map(|row| row.get(index))
                .map(|cell| width_of(&cell.text))
                .chain(std::iter::once(width_of(column.header)))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let floor: Vec<usize> = columns
        .iter()
        .zip(&natural)
        .map(|(column, natural)| match column.shrink {
            Some(_) => column.min.min(*natural),
            None => *natural,
        })
        .collect();

    let span = |kept: &[bool], widths: &[usize]| -> usize {
        let count = kept.iter().filter(|keep| **keep).count();
        kept.iter()
            .zip(widths)
            .filter(|(keep, _)| **keep)
            .map(|(_, width)| *width)
            .sum::<usize>()
            + GAP * count.saturating_sub(1)
    };
    let ordered = |key: fn(&Column) -> Option<u8>| {
        let mut order: Vec<usize> = (0..columns.len())
            .filter(|index| key(&columns[*index]).is_some())
            .collect();
        order.sort_by_key(|index| key(&columns[*index]));
        order
    };

    let mut kept = vec![true; columns.len()];
    for index in ordered(|column| column.drop) {
        if span(&kept, &floor) <= width {
            break;
        }
        kept[index] = false;
    }

    // What the survivors did not need is shared out a column at a time rather
    // than handed to the first claimant: the title and the branch both start
    // far below the width they want, and a listing where one of them is whole
    // and the other is an ellipsis reads worse than one where both are close.
    let mut widths = floor;
    let growable: Vec<usize> = ordered(|column| column.shrink)
        .into_iter()
        .filter(|index| kept[*index])
        .collect();
    let mut surplus = width.saturating_sub(span(&kept, &widths));
    while surplus > 0 {
        let before = surplus;
        for index in &growable {
            if surplus == 0 {
                break;
            }
            if widths[*index] < natural[*index] {
                widths[*index] += 1;
                surplus -= 1;
            }
        }
        if surplus == before {
            break;
        }
    }

    kept.into_iter()
        .zip(widths)
        .map(|(keep, width)| keep.then_some(width))
        .collect()
}

/// Lay the table out for `width` and paint it: the header line, then one line
/// per row, each without its newline so a caller can interleave its own lines
/// between them.
pub fn lines(style: Style, width: usize, columns: &[Column], rows: &[Vec<Cell>]) -> Vec<String> {
    let widths = plan(columns, rows, width);
    let kept: Vec<usize> = (0..columns.len())
        .filter(|i| widths[*i].is_some())
        .collect();
    if kept.is_empty() {
        return Vec::new();
    }
    let header: Vec<Cell> = columns
        .iter()
        .map(|column| Cell::painted(Role::Heading, column.header))
        .collect();
    std::iter::once(&header)
        .chain(rows)
        .map(|cells| line(style, &widths, &kept, cells))
        .collect()
}

/// Lay the table out and join it into one block.
pub fn render(style: Style, width: usize, columns: &[Column], rows: &[Vec<Cell>]) -> String {
    lines(style, width, columns, rows)
        .into_iter()
        .map(|line| line + "\n")
        .collect()
}

/// One line. Padding is written unpainted and the trailing column is not
/// padded at all, so a row never ends in styled or trailing whitespace.
fn line(style: Style, widths: &[Option<usize>], kept: &[usize], cells: &[Cell]) -> String {
    let blank = Cell::plain("");
    let mut out = String::new();
    for (position, index) in kept.iter().enumerate() {
        let column = widths[*index].unwrap_or(0);
        let cell = cells.get(*index).unwrap_or(&blank);
        let text = fit(&cell.text, column);
        let painted = match cell.role {
            Some(role) => style.paint(role, &text),
            None => crate::util::display_safe_block(&text),
        };
        out.push_str(&painted);
        if position + 1 < kept.len() {
            let pad = column.saturating_sub(width_of(&text)) + GAP;
            out.push_str(&" ".repeat(pad));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::ColorChoice;

    fn columns_fixture() -> Vec<Column> {
        vec![
            Column {
                header: "HANDLE",
                min: 0,
                shrink: None,
                drop: None,
            },
            Column {
                header: "TITLE",
                min: 6,
                shrink: Some(0),
                drop: Some(1),
            },
            Column {
                header: "MODE",
                min: 0,
                shrink: None,
                drop: Some(0),
            },
        ]
    }

    fn row(handle: &str, title: &str, mode: &str) -> Vec<Cell> {
        vec![Cell::plain(handle), Cell::plain(title), Cell::plain(mode)]
    }

    #[test]
    fn a_handle_keeps_its_width_while_the_title_gives_up_its_own() {
        let columns = columns_fixture();
        let rows = vec![row(
            "@storage-cleanup",
            "a long task title that will not fit",
            "headless",
        )];
        let text = render(Style::plain(), 40, &columns, &rows);
        for line in text.lines() {
            assert!(width_of(line) <= 40, "{line:?} in {text}");
        }
        assert!(text.contains("@storage-cleanup"), "{text}");
        assert!(text.contains('…'), "{text}");
        assert!(text.contains("headless"), "{text}");
    }

    #[test]
    fn the_lowest_value_column_leaves_before_the_title_becomes_unreadable() {
        let columns = columns_fixture();
        let rows = vec![row("@storage-cleanup", "a long task title", "headless")];
        let text = render(Style::plain(), 28, &columns, &rows);
        assert!(!text.contains("headless"), "{text}");
        assert!(!text.contains("MODE"), "{text}");
        for line in text.lines() {
            assert!(width_of(line) <= 28, "{line:?} in {text}");
        }
    }

    #[test]
    fn a_dropped_column_hands_its_width_back_to_the_title() {
        let columns = columns_fixture();
        let rows = vec![row("@a", "a title of some length here", "headless")];
        // HANDLE takes its header's 6 columns; the floor layout is 6 + 6 + 8
        // with two gaps, so 24 is exactly wide enough to keep MODE and 23 is not.
        let kept = render(Style::plain(), 24, &columns, &rows);
        let dropped = render(Style::plain(), 23, &columns, &rows);
        assert!(kept.contains("headless"), "{kept}");
        assert!(!dropped.contains("headless"), "{dropped}");
        // Losing MODE and its gap hands nine columns to the title, which is the
        // only column left that wanted more.
        assert_eq!(kept.lines().nth(1).unwrap(), "@a      a tit…  headless");
        assert_eq!(dropped.lines().nth(1).unwrap(), "@a      a title of som…");
    }

    #[test]
    fn everything_fits_untouched_when_the_terminal_is_wide_enough() {
        let columns = columns_fixture();
        let rows = vec![row("@a", "short", "cmux")];
        let text = render(Style::plain(), 200, &columns, &rows);
        assert!(!text.contains('…'), "{text}");
        assert_eq!(
            text.lines().next().unwrap().trim_end(),
            "HANDLE  TITLE  MODE"
        );
        assert_eq!(
            text.lines().nth(1).unwrap().trim_end(),
            "@a      short  cmux"
        );
    }

    #[test]
    fn rows_never_end_in_styled_padding_and_unstyled_cells_stay_bare() {
        let style = Style::resolve(Some(ColorChoice::Always), false, false, true);
        let columns = columns_fixture();
        let rows = vec![vec![
            Cell::painted(Role::Agent, "@a"),
            Cell::plain("short"),
            Cell::plain("cmux"),
        ]];
        let text = render(style, 200, &columns, &rows);
        let data = text.lines().nth(1).unwrap();
        assert!(data.starts_with("\x1b[1;36m@a\x1b[0m"), "{data:?}");
        assert!(data.ends_with("cmux"), "{data:?}");
        assert!(!data.contains("\x1b[0m  \x1b["), "{data:?}");
    }

    #[test]
    fn a_cell_that_escaped_its_caller_is_neutralized_rather_than_painted() {
        let style = Style::resolve(Some(ColorChoice::Always), false, false, true);
        let columns = columns_fixture();
        let rows = vec![vec![
            Cell::painted(Role::Agent, "a\x1b[2Jb"),
            Cell::plain("c\x1b[2Jd"),
            Cell::plain("cmux"),
        ]];
        let text = render(style, 200, &columns, &rows);
        let data = text.lines().nth(1).unwrap();
        assert!(!data.contains('\x1b'), "{data:?}");
        assert_eq!(data, "a\\x1b[2Jb  c\\x1b[2Jd  cmux");
    }

    #[test]
    fn an_explicit_width_wins_and_an_unreported_one_falls_back_to_eighty() {
        assert_eq!(resolve(Some(120), || Some(200)), 120);
        assert_eq!(resolve(None, || Some(200)), 200);
        assert_eq!(resolve(None, || None), DEFAULT_COLUMNS);
        // A terminal too narrow to lay out is not honored in either source.
        assert_eq!(resolve(Some(4), || None), MIN_COLUMNS);
        assert_eq!(resolve(None, || Some(4)), MIN_COLUMNS);
    }

    #[test]
    fn an_ellipsis_is_never_the_only_thing_a_column_shows() {
        assert_eq!(fit("abcdef", 6), "abcdef");
        assert_eq!(fit("abcdef", 5), "abcd…");
        assert_eq!(fit("abcdef", 1), "…");
        assert_eq!(fit("abcdef", 0), "");
    }
}
