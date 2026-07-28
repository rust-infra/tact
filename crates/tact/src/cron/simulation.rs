//! Standalone simulation of the Tact cron scheduling engine.
//!
//! This program simulates the full lifecycle: create tasks, tick through
//! simulated time, fire matching tasks, handle one-shot cleanup, and
//! demonstrate durable vs session-scoped behavior.
//!
//! Run with: cargo test -p tact --lib cron::simulation -- --nocapture
//!
//! Since the scheduling engine is not yet implemented, this test acts as a
//! design prototype showing the expected behavior.

#[cfg(test)]
mod simulation_tests {
    use chrono::{DateTime, Datelike, NaiveDateTime, Timelike, Utc};

    // ── Minimal cron matcher (field-level) ──────────────────────────

    #[derive(Debug, Clone, PartialEq)]
    enum CronField {
        Any,
        Exact(u32),
    }

    impl CronField {
        fn from_str(s: &str) -> Option<Self> {
            if s == "*" {
                return Some(Self::Any);
            }
            s.parse::<u32>().ok().map(Self::Exact)
        }

        fn matches(&self, v: u32) -> bool {
            match self {
                Self::Any => true,
                Self::Exact(n) => *n == v,
            }
        }
    }

    /// 5-field cron: minute hour day-of-month month day-of-week
    #[derive(Debug, Clone)]
    struct CronExpr {
        minute: CronField,
        hour: CronField,
        dom: CronField,
        month: CronField,
        dow: CronField,
    }

    impl CronExpr {
        fn parse(expr: &str) -> Option<Self> {
            let parts: Vec<&str> = expr.split_whitespace().collect();
            if parts.len() != 5 {
                return None;
            }
            Some(Self {
                minute: CronField::from_str(parts[0])?,
                hour: CronField::from_str(parts[1])?,
                dom: CronField::from_str(parts[2])?,
                month: CronField::from_str(parts[3])?,
                dow: CronField::from_str(parts[4])?,
            })
        }

        fn matches(&self, dt: &DateTime<Utc>) -> bool {
            self.minute.matches(dt.minute())
                && self.hour.matches(dt.hour())
                && self.dom.matches(dt.day())
                && self.month.matches(dt.month())
                && self.dow.matches(dt.weekday().num_days_from_sunday())
        }

        /// Compute next fire time after `after` (up to ~1 year).
        fn next_after(&self, after: &DateTime<Utc>) -> Option<DateTime<Utc>> {
            let mut candidate = *after + chrono::Duration::minutes(1);
            // Cap search at 1 year
            let limit = *after + chrono::Duration::days(366);
            while candidate <= limit {
                if self.matches(&candidate) {
                    return Some(candidate);
                }
                candidate += chrono::Duration::minutes(1);
            }
            None
        }
    }

    // ── Task model (mirrors ScheduledTaskRecord) ────────────────────

    #[derive(Debug, Clone)]
    struct SimTask {
        id: String,
        cron: String,
        prompt: String,
        recurring: bool,
        durable: bool,
    }

    // ── Scheduling engine simulation ────────────────────────────────

    struct SimScheduler {
        tasks: Vec<SimTask>,
        fired_log: Vec<String>,
    }

    impl SimScheduler {
        fn new() -> Self {
            Self {
                tasks: vec![],
                fired_log: vec![],
            }
        }

        fn create(
            &mut self,
            id: &str,
            cron: &str,
            prompt: &str,
            recurring: bool,
            durable: bool,
        ) {
            self.tasks.push(SimTask {
                id: id.to_string(),
                cron: cron.to_string(),
                prompt: prompt.to_string(),
                recurring,
                durable,
            });
        }

        /// Tick the clock forward to `now`, firing any matching tasks.
        fn tick(&mut self, now: &DateTime<Utc>) -> Vec<String> {
            let mut fired_this_tick: Vec<String> = vec![];
            let mut to_delete: Vec<usize> = vec![];

            for (i, task) in self.tasks.iter().enumerate() {
                let expr = match CronExpr::parse(&task.cron) {
                    Some(e) => e,
                    None => continue,
                };
                if expr.matches(now) {
                    let msg = format!(
                        "🔥 FIRED [{id}] ({cron}) → \"{prompt}\" at {now}",
                        id = task.id,
                        cron = task.cron,
                        prompt = task.prompt,
                        now = now.format("%Y-%m-%d %H:%M:%S"),
                    );
                    fired_this_tick.push(msg.clone());
                    self.fired_log.push(msg);

                    if !task.recurring {
                        to_delete.push(i);
                    }
                }
            }

            // Delete one-shot tasks in reverse index order
            for i in to_delete.into_iter().rev() {
                let removed = self.tasks.remove(i);
                self.fired_log.push(format!(
                    "🗑  DELETE one-shot [{id}] ({cron}) — already fired",
                    id = removed.id,
                    cron = removed.cron
                ));
            }

            fired_this_tick
        }

        /// Simulate session restart: drop session-scoped tasks.
        fn restart_session(&mut self) {
            let before = self.tasks.len();
            self.tasks.retain(|t| t.durable);
            self.fired_log.push(format!(
                "🔄 SESSION RESTART — dropped {} session tasks, kept {} durable",
                before - self.tasks.len(),
                self.tasks.len()
            ));
        }
    }

    // ── Tests ───────────────────────────────────────────────────────

    #[test]
    fn simulate_cron_full_lifecycle() {
        let mut s = SimScheduler::new();
        let start =
            DateTime::<Utc>::from_naive_utc_and_offset(
                NaiveDateTime::parse_from_str("2026-07-28 08:59:00", "%Y-%m-%d %H:%M:%S").unwrap(),
                Utc,
            );

        // ── Phase 1: Create tasks ──────────────────────────────────

        // Task A: every day at 09:00, recurring, session-scoped
        s.create("A", "0 9 * * *", "Daily standup", true, false);
        // Task B: every minute, one-shot, durable
        s.create("B", "* * * * *", "Check CI once", false, true);
        // Task C: Mondays 10:00, recurring, durable
        s.create("C", "0 10 * * 1", "Weekly PR review", true, true);

        eprintln!("=== PHASE 1: Created 3 tasks ===");
        eprintln!("  [A] 0 9 * * *     recurring/session  \"Daily standup\"");
        eprintln!("  [B] * * * * *     one-shot/durable   \"Check CI once\"");
        eprintln!("  [C] 0 10 * * 1    recurring/durable  \"Weekly PR review\"");

        // ── Phase 2: Step-by-step tick simulation ──────────────────

        let mut events: Vec<String> = vec![];
        let mut t = start;

        // Simulate 3 days, 1 tick per minute (compressed log)
        let end = start + chrono::Duration::days(3);
        while t < end {
            t += chrono::Duration::minutes(1);
            let fired = s.tick(&t);
            for f in &fired {
                events.push(f.clone());
            }
        }

        eprintln!("  Simulated {} minutes, {} fire events logged", 
                  (end - start).num_minutes(), events.len());

        // Task B fires every minute → one-shot → should fire exactly once then delete
        let b_fires: Vec<_> = events.iter().filter(|e| e.contains("[B]")).collect();
        assert_eq!(b_fires.len(), 1, "Task B is one-shot, should fire exactly once");
        assert_eq!(
            s.tasks.iter().filter(|t| t.id == "B").count(),
            0,
            "Task B should be removed from task list after firing"
        );
        assert!(
            s.fired_log.iter().any(|e| e.contains("DELETE") && e.contains("[B]")),
            "Task B should be auto-deleted after firing"
        );

        // Task A fires every day at 09:00 → 3 times in 3 days
        let a_fires: Vec<_> = events.iter().filter(|e| e.contains("[A]")).collect();
        assert_eq!(a_fires.len(), 3, "Task A should fire 3 times (once per day at 09:00)");

        // Task C: 0 10 * * 1 — 2026-07-28 is Tuesday, so next Monday is 2026-08-03
        // Within 3 days (ending 2026-07-31), no Monday → C never fires
        let c_fires: Vec<_> = events.iter().filter(|e| e.contains("[C]")).collect();
        assert_eq!(c_fires.len(), 0, "Task C should NOT fire within Tue→Fri window");

        eprintln!("  A fires: {}", a_fires.len());
        eprintln!("  B fires: {}", b_fires.len());
        eprintln!("  C fires: {}", c_fires.len());

        // ── Phase 3: Session restart ───────────────────────────────

        eprintln!("\n=== PHASE 3: Session restart ===");
        s.restart_session();
        // After restart: only durable tasks survive → task C survives, A is dropped
        assert_eq!(
            s.tasks.iter().filter(|t| t.id == "A").count(),
            0,
            "Task A (session-scoped) should be dropped on restart"
        );
        assert_eq!(
            s.tasks.iter().filter(|t| t.id == "C").count(),
            1,
            "Task C (durable) should survive restart"
        );
        eprintln!("  After restart: {:?}", s.tasks.iter().map(|t| &t.id).collect::<Vec<_>>());

        // ── Phase 4: Next-fire preview ─────────────────────────────

        eprintln!("\n=== PHASE 4: Next-fire preview (after restart) ===");
        let now = start + chrono::Duration::days(4); // 2026-08-01
        for task in &s.tasks {
            let expr = CronExpr::parse(&task.cron).unwrap();
            if let Some(next) = expr.next_after(&now) {
                eprintln!(
                    "  [{id}] {cron} → next fire: {next}",
                    id = task.id,
                    cron = task.cron,
                    next = next.format("%Y-%m-%d %H:%M:%S (%A)")
                );
            } else {
                eprintln!("  [{id}] {cron} → no fire within 1 year", id = task.id, cron = task.cron);
            }
        }

        // ── Phase 5: Bad input / edge cases ────────────────────────

        eprintln!("\n=== PHASE 5: Edge cases ===");

        // Invalid cron expression
        let bad = CronExpr::parse("99 99 99 99 99");
        eprintln!("  Parse '99 99 ... ' → {bad:?}");
        assert!(bad.is_some(), "should parse as Exact(99) values — matches nothing");

        // Missing fields
        let too_short = CronExpr::parse("0 9 *");
        eprintln!("  Parse '0 9 *' → {too_short:?}");
        assert!(too_short.is_none(), "too few fields should fail");

        // Exactly midnight
        let midnight_expr = CronExpr::parse("0 0 * * *").unwrap();
        let mid = DateTime::<Utc>::from_naive_utc_and_offset(
            NaiveDateTime::parse_from_str("2026-08-01 00:00:00", "%Y-%m-%d %H:%M:%S").unwrap(),
            Utc,
        );
        assert!(midnight_expr.matches(&mid));

        // Weekday matching: 0 = Sunday, 1 = Monday, ..., 7 = Sunday (cron convention)
        let mon_expr = CronExpr::parse("0 9 * * 1").unwrap();
        let tue = DateTime::<Utc>::from_naive_utc_and_offset(
            NaiveDateTime::parse_from_str("2026-07-28 09:00:00", "%Y-%m-%d %H:%M:%S").unwrap(),
            Utc,
        );
        assert!(!mon_expr.matches(&tue), "2026-07-28 is Tuesday, should not match Monday expr");

        eprintln!("  All edge cases passed.");
    }

    /// Simulate rapid-fire: a * * * * * (every-minute) recurring task for 5 minutes.
    #[test]
    fn simulate_every_minute_recurring() {
        let mut s = SimScheduler::new();
        s.create("X", "* * * * *", "Heartbeat", true, false);

        let t0 =
            DateTime::<Utc>::from_naive_utc_and_offset(
                NaiveDateTime::parse_from_str("2026-08-01 12:00:00", "%Y-%m-%d %H:%M:%S").unwrap(),
                Utc,
            );

        let mut t = t0;
        let mut fires = 0;
        for _ in 0..5 {
            t += chrono::Duration::minutes(1);
            if !s.tick(&t).is_empty() {
                fires += 1;
            }
        }
        assert_eq!(fires, 5, "Recurring every-minute should fire 5 times in 5 minutes");
        assert_eq!(s.tasks.len(), 1, "Recurring task should not be deleted");
    }

    /// Simulate the idempotency issue: what happens when two ticks land on the same minute?
    #[test]
    fn simulate_duplicate_tick_does_not_refire_one_shot() {
        let mut s = SimScheduler::new();
        s.create("Y", "30 14 * * *", "Once at 14:30", false, false);

        let t =
            DateTime::<Utc>::from_naive_utc_and_offset(
                NaiveDateTime::parse_from_str("2026-08-01 14:30:00", "%Y-%m-%d %H:%M:%S").unwrap(),
                Utc,
            );

        // First tick fires and deletes
        let f1 = s.tick(&t);
        assert_eq!(f1.len(), 1);
        assert_eq!(s.tasks.len(), 0, "One-shot deleted after first fire");

        // Second tick at same time should do nothing
        let f2 = s.tick(&t);
        assert!(f2.is_empty(), "Task already deleted, should not refire");
    }
}
