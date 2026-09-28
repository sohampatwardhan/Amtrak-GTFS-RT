//! The CTrail Hartford Line connecting-train board for New Haven Union.
//!
//! The board is a public HTML page listing connecting Amtrak trains with their track. Only rows
//! whose service column reads `Amtrak` and whose track is a platform token are kept.

use super::{is_track_label, normalize_train_number};
use scraper::{Html, Selector};

/// Marker that must appear in a real board page; anything else (an error or maintenance page)
/// is treated as a failed fetch so the station's earlier rows age out instead of being cleared.
pub const BOARD_MARKER: &str = "status-table";

/// Amtrak rows from the Hartford Line connecting-train table: `(train number, track)`.
pub fn parse_hartford_board(html: &str) -> Vec<(String, String)> {
    let document = Html::parse_document(html);
    let Ok(rows) = Selector::parse("#status-table tbody tr") else {
        return Vec::new();
    };
    let Ok(cells) = Selector::parse("td") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for row in document.select(&rows) {
        let fields: Vec<String> = row
            .select(&cells)
            .map(|cell| {
                cell.text()
                    .collect::<String>()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        if fields.len() < 6 || !fields[1].eq_ignore_ascii_case("amtrak") {
            continue;
        }
        let Some(train_number) = normalize_train_number(&fields[2]) else {
            continue;
        };
        let track = fields[5].trim().to_uppercase();
        if !is_track_label(&track) {
            continue;
        }
        out.push((train_number, track));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_keeps_amtrak_rows() {
        let html = include_str!("../../../fixtures/tracks/hartford_board.html");
        assert!(html.contains(BOARD_MARKER));
        let rows = parse_hartford_board(html);
        assert_eq!(
            rows,
            vec![
                ("141".into(), "3".into()),
                ("2151".into(), "1".into()),
                ("2190".into(), "2".into()),
                ("66".into(), "1".into()),
            ]
        );
    }
}
