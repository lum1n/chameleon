//! The agent's to-do list (Claude's `TodoWrite`, Codex's turn plan, an ACP
//! plan), pinned under the transcript while it has work left. Done entries
//! are ticked, the one in progress is lit, and a long list shows the rows
//! around it.
//!
//! ```text
//!  plan 1/3
//!  ✓ Read the failing test
//!  ◐ Fix the parser
//!  ○ Run the suite
//! ```

use acc_core::event::{Plan, PlanStatus};

use crate::config::Theme;
use crate::style::{Line, Span, Style};
use crate::wrap::truncate;

/// Entry rows shown at most; the rest are counted.
pub const MAX_ENTRIES: usize = 5;

/// The pinned lines for `plan`, `w` columns wide: none once every entry
/// is done.
pub fn lines(plan: &Plan, theme: &Theme, w: usize) -> Vec<Line> {
    let entries = &plan.entries;
    let done = entries
        .iter()
        .filter(|e| e.status == PlanStatus::Completed)
        .count();
    if done == entries.len() {
        return Vec::new();
    }
    let muted = Style::new().fg(theme.muted);
    // Start one row above the first unfinished entry, so the last done one
    // stays in sight, without leaving rows empty at the end.
    let first = entries
        .iter()
        .position(|e| e.status != PlanStatus::Completed)
        .unwrap_or(0);
    let start = first
        .saturating_sub(1)
        .min(entries.len().saturating_sub(MAX_ENTRIES));
    let shown = &entries[start..(start + MAX_ENTRIES).min(entries.len())];
    let hidden = entries.len() - shown.len();

    let mut head = vec![
        Span::new("plan ", Style::new().fg(theme.accent).bold()),
        Span::new(format!("{done}/{}", entries.len()), muted),
    ];
    if hidden > 0 {
        head.push(Span::new(format!(" · {hidden} more"), muted));
    }
    let mut out = vec![Line::new(head)];
    for e in shown {
        let (glyph, glyph_style, text_style) = match e.status {
            PlanStatus::Completed => ("✓ ", Style::new().fg(theme.done), muted),
            PlanStatus::InProgress => (
                "◐ ",
                Style::new().fg(theme.accent),
                Style::new().fg(theme.agent).bold(),
            ),
            PlanStatus::Pending => ("○ ", muted, Style::new().fg(theme.agent)),
        };
        out.push(Line::new(vec![
            Span::new(glyph, glyph_style),
            Span::new(truncate(e.content.trim(), w.saturating_sub(2)), text_style),
        ]));
    }
    out
}

#[cfg(test)]
mod tests {
    use acc_core::event::PlanEntry;

    use super::*;

    fn plan(statuses: &[PlanStatus]) -> Plan {
        Plan {
            entries: statuses
                .iter()
                .enumerate()
                .map(|(i, s)| PlanEntry {
                    content: format!("step {i}"),
                    status: *s,
                    priority: None,
                })
                .collect(),
            explanation: None,
        }
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::text).collect()
    }

    #[test]
    fn a_finished_plan_shows_nothing() {
        use PlanStatus::*;
        assert!(lines(&plan(&[Completed, Completed]), &Theme::default(), 40).is_empty());
        assert!(lines(&plan(&[]), &Theme::default(), 40).is_empty());
    }

    #[test]
    fn a_long_plan_shows_the_rows_around_the_work() {
        use PlanStatus::*;
        let p = plan(&[
            Completed, Completed, Completed, InProgress, Pending, Pending, Pending, Pending,
        ]);
        assert_eq!(
            texts(&lines(&p, &Theme::default(), 40)),
            [
                "plan 3/8 · 3 more",
                "✓ step 2",
                "◐ step 3",
                "○ step 4",
                "○ step 5",
                "○ step 6",
            ]
        );
        // Near the end the window stops at the last entry.
        let p = plan(&[
            Completed, Completed, Completed, Completed, Completed, Completed, Pending,
        ]);
        assert_eq!(
            texts(&lines(&p, &Theme::default(), 40))[1..],
            ["✓ step 2", "✓ step 3", "✓ step 4", "✓ step 5", "○ step 6"]
        );
    }
}
