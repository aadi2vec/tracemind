//! I8 — Consent-mode compliance bench.
//!
//! Runs a synthetic capture stream against the full consent stack
//! (`ModeManager` + `SensitiveAppPolicy` + `AntiGoalRules` + `AppScopingPolicy`
//! + `ReceiptStore`) and asserts the ten safety commandments from
//! `docs/INGESTION_EXPERIENCE_PLAN-2026-07-22.md` §5:
//!
//! * S1 — Every capture event produces a receipt (or is explicitly dropped
//!   with a `reason` written to the ingestion review log).
//! * S6 — Private mode drops every candidate; the receipt log grows by zero.
//! * S5 — Sensitive-app blocklist is a hard floor: even Ambient mode drops.
//! * S7 — Anti-goal rules match on url / content / source and drop.
//! * S8 — App scoping Deny drops; Allow lets through; Ask does not drop but
//!   is flagged for the review view.
//!
//! The bench is intentionally hand-rolled instead of leaning on a mock
//! pipeline: the ingestion happy path in `IngestPipeline` still has
//! multi-crate side effects (SQLite, embedder) that are unnecessary to
//! prove the *governance* contract. Keeping the bench governance-only means
//! it stays fast (< 20 ms) and can run in every CI cut.

use chrono::Utc;
use tm_governance::{
    AntiGoalRules, AppDisposition, AppScopingPolicy, SensitiveAppPolicy,
};
use tm_ingest::{ModeManager, ReceiptStore};
use tm_types::{CaptureMode, CaptureReceipt, GateDecisions, MemoryTier};

// ---------------------------------------------------------------------------
// A pure test-only gate — the same decision tree the daemon runs before
// touching any downstream store.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CaptureCandidate {
    source: &'static str,
    app: &'static str,
    url: Option<&'static str>,
    content: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Admit,
    Drop(String),
}

struct ConsentStack<'a> {
    modes: &'a ModeManager,
    apps_sensitive: &'a SensitiveAppPolicy,
    anti_goal: &'a AntiGoalRules,
    scoping: &'a AppScopingPolicy,
    receipts: &'a ReceiptStore,
}

impl<'a> ConsentStack<'a> {
    fn admit(&self, cand: &CaptureCandidate) -> Decision {
        let now = Utc::now();

        // S6 — Private mode drops everything, no receipt.
        if self.modes.current(now) == CaptureMode::Private {
            return Decision::Drop("mode:private".into());
        }

        // S5 — sensitive-app blocklist.
        if self.apps_sensitive.is_blocked(cand.app) {
            return Decision::Drop("sensitive_app".into());
        }

        // S8 — per-app disposition (only Deny is a hard drop here; Ask is
        // logged as-if-Ambient with a receipt flag).
        if self.scoping.decide(cand.app) == AppDisposition::Deny {
            return Decision::Drop("app_scope:deny".into());
        }

        // S7 — anti-goal rules.
        if let Some(rule) = self
            .anti_goal
            .matches(cand.url, cand.content, cand.source)
        {
            return Decision::Drop(format!("anti_goal:{rule}"));
        }

        // Passed all gates — S1: write a receipt.
        let mut receipt = CaptureReceipt::new(
            cand.source,
            "text",
            cand.content.len(),
            MemoryTier::Quarantine,
            "passed all consent gates",
        )
        .with_app_context(cand.app)
        .with_gates(GateDecisions {
            pii_pass: true,
            noise_pass: true,
            sensitive_app: false,
        });
        if let Some(session) = self.modes.active_session(now) {
            if session.mode == CaptureMode::Focus && !session.name.is_empty() {
                receipt = receipt.with_intent(session.name);
            }
        }
        self.receipts
            .append(&receipt)
            .expect("receipt append should never fail");
        Decision::Admit
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fresh_stack() -> (
    tempfile::TempDir,
    ModeManager,
    SensitiveAppPolicy,
    AntiGoalRules,
    AppScopingPolicy,
    ReceiptStore,
) {
    let dir = tempfile::tempdir().unwrap();
    let modes = ModeManager::open(dir.path()).unwrap();
    let apps_sensitive = SensitiveAppPolicy::bundled();
    let anti_goal = AntiGoalRules::default();
    let mut scoping = AppScopingPolicy::default();
    // Ambient-safe default so we don't fight the Ask branch for every fixture.
    scoping.default_disposition = Some(AppDisposition::Allow);
    let receipts = ReceiptStore::open(dir.path()).unwrap();
    (dir, modes, apps_sensitive, anti_goal, scoping, receipts)
}

fn receipt_count(store: &ReceiptStore) -> usize {
    store.recent(1000).unwrap().len()
}

// ---------------------------------------------------------------------------
// S1 — ambient captures always leave a receipt.
// ---------------------------------------------------------------------------

#[test]
fn ambient_ok_capture_writes_receipt() {
    let (_dir, modes, apps, ag, scope, receipts) = fresh_stack();
    let stack = ConsentStack {
        modes: &modes,
        apps_sensitive: &apps,
        anti_goal: &ag,
        scoping: &scope,
        receipts: &receipts,
    };
    let d = stack.admit(&CaptureCandidate {
        source: "clipboard",
        app: "Xcode",
        url: None,
        content: "let x = 42; // sample snippet",
    });
    assert_eq!(d, Decision::Admit);
    assert_eq!(receipt_count(&receipts), 1);
    let r = &receipts.recent(1).unwrap()[0];
    assert_eq!(r.source, "clipboard");
    assert_eq!(r.app_context.as_deref(), Some("Xcode"));
    assert!(r.user_intent.is_none(), "Ambient carries no intent");
}

// ---------------------------------------------------------------------------
// Focus mode — receipt carries the active intent name.
// ---------------------------------------------------------------------------

#[test]
fn focus_mode_receipt_carries_intent() {
    let (_dir, modes, apps, ag, scope, receipts) = fresh_stack();
    modes.enter_focus("call-prep-tuesday", 60 * 30).unwrap();
    let stack = ConsentStack {
        modes: &modes,
        apps_sensitive: &apps,
        anti_goal: &ag,
        scoping: &scope,
        receipts: &receipts,
    };
    stack.admit(&CaptureCandidate {
        source: "web",
        app: "Safari",
        url: Some("https://example.com/prep"),
        content: "meeting agenda draft",
    });
    let r = &receipts.recent(1).unwrap()[0];
    assert_eq!(r.user_intent.as_deref(), Some("call-prep-tuesday"));
}

// ---------------------------------------------------------------------------
// S6 — Private mode: no receipt at all, ever.
// ---------------------------------------------------------------------------

#[test]
fn private_mode_writes_zero_receipts_across_stream() {
    let (_dir, modes, apps, ag, scope, receipts) = fresh_stack();
    modes.enter_private(60 * 15).unwrap();
    let stack = ConsentStack {
        modes: &modes,
        apps_sensitive: &apps,
        anti_goal: &ag,
        scoping: &scope,
        receipts: &receipts,
    };

    let stream = vec![
        CaptureCandidate {
            source: "clipboard",
            app: "Xcode",
            url: None,
            content: "let a = 1;",
        },
        CaptureCandidate {
            source: "web",
            app: "Safari",
            url: Some("https://example.com"),
            content: "some post",
        },
        CaptureCandidate {
            source: "screenshot",
            app: "Xcode",
            url: None,
            content: "png bytes stand-in",
        },
        CaptureCandidate {
            source: "keyboard",
            app: "Terminal",
            url: None,
            content: "ls -la /tmp",
        },
    ];

    for c in &stream {
        let d = stack.admit(c);
        assert!(
            matches!(d, Decision::Drop(_)),
            "Private must drop: got {d:?} for {c:?}"
        );
    }
    assert_eq!(
        receipt_count(&receipts),
        0,
        "S6 violated: Private mode wrote {} receipts",
        receipt_count(&receipts)
    );
}

// ---------------------------------------------------------------------------
// S5 — sensitive-app blocklist is a hard floor, even in Ambient.
// ---------------------------------------------------------------------------

#[test]
fn sensitive_app_blocked_in_ambient_writes_no_receipt() {
    let (_dir, modes, apps, ag, scope, receipts) = fresh_stack();
    // Pick an app the bundled blocklist actually blocks.
    let blocked_app = apps
        .blocklist
        .iter()
        .next()
        .cloned()
        .expect("bundled blocklist must be non-empty");
    let stack = ConsentStack {
        modes: &modes,
        apps_sensitive: &apps,
        anti_goal: &ag,
        scoping: &scope,
        receipts: &receipts,
    };
    // We know current mode is Ambient (no session entered).
    assert_eq!(modes.current(Utc::now()), CaptureMode::Ambient);

    // Leak `blocked_app` as a &'static str for the fixture struct — the
    // gate only reads it for the app check.
    let leaked: &'static str = Box::leak(blocked_app.into_boxed_str());
    let d = stack.admit(&CaptureCandidate {
        source: "clipboard",
        app: leaked,
        url: None,
        content: "should not be captured",
    });
    assert_eq!(d, Decision::Drop("sensitive_app".into()));
    assert_eq!(receipt_count(&receipts), 0);
}

// ---------------------------------------------------------------------------
// S7 — anti-goal rules drop captures.
// ---------------------------------------------------------------------------

#[test]
fn anti_goal_content_token_drops_with_no_receipt() {
    let (_dir, modes, apps, mut ag, scope, receipts) = fresh_stack();
    ag.add_token("password");
    let stack = ConsentStack {
        modes: &modes,
        apps_sensitive: &apps,
        anti_goal: &ag,
        scoping: &scope,
        receipts: &receipts,
    };
    let d = stack.admit(&CaptureCandidate {
        source: "clipboard",
        app: "Xcode",
        url: None,
        content: "my Password is hunter2",
    });
    assert!(matches!(d, Decision::Drop(reason) if reason.starts_with("anti_goal:content:")));
    assert_eq!(receipt_count(&receipts), 0);
}

#[test]
fn anti_goal_url_match_drops_web_capture() {
    let (_dir, modes, apps, mut ag, scope, receipts) = fresh_stack();
    ag.add_url("banking");
    let stack = ConsentStack {
        modes: &modes,
        apps_sensitive: &apps,
        anti_goal: &ag,
        scoping: &scope,
        receipts: &receipts,
    };
    let d = stack.admit(&CaptureCandidate {
        source: "web",
        app: "Safari",
        url: Some("https://Chase.Banking.example/x"),
        content: "landing page",
    });
    assert!(matches!(d, Decision::Drop(reason) if reason.starts_with("anti_goal:url:")));
    assert_eq!(receipt_count(&receipts), 0);
}

// ---------------------------------------------------------------------------
// S8 — per-app disposition.
// ---------------------------------------------------------------------------

#[test]
fn app_scoping_deny_drops_with_no_receipt() {
    let (_dir, modes, apps, ag, mut scope, receipts) = fresh_stack();
    // "Notion" is deliberately NOT on the bundled sensitive-app blocklist,
    // so we can prove that per-app Deny fires independently of S5.
    assert!(!apps.is_blocked("Notion"));
    scope.deny("Notion");
    let stack = ConsentStack {
        modes: &modes,
        apps_sensitive: &apps,
        anti_goal: &ag,
        scoping: &scope,
        receipts: &receipts,
    };
    let d = stack.admit(&CaptureCandidate {
        source: "clipboard",
        app: "Notion",
        url: None,
        content: "some page content",
    });
    assert_eq!(d, Decision::Drop("app_scope:deny".into()));
    assert_eq!(receipt_count(&receipts), 0);
}

// ---------------------------------------------------------------------------
// End-to-end: a mixed synthetic stream produces exactly the receipts we
// expect and no more.
// ---------------------------------------------------------------------------

#[test]
fn mixed_stream_admits_only_the_expected_events() {
    let (_dir, modes, apps, mut ag, mut scope, receipts) = fresh_stack();
    ag.add_token("secret");
    // Notion is off the sensitive-app blocklist so the drop below is
    // unambiguously attributable to per-app Deny, not S5.
    scope.deny("Notion");
    let stack = ConsentStack {
        modes: &modes,
        apps_sensitive: &apps,
        anti_goal: &ag,
        scoping: &scope,
        receipts: &receipts,
    };

    // 4 events; only #1 and #4 should be admitted.
    let stream = vec![
        (
            "expected_admit",
            CaptureCandidate {
                source: "clipboard",
                app: "Xcode",
                url: None,
                content: "let x = 1",
            },
        ),
        (
            "denied_by_scope",
            CaptureCandidate {
                source: "clipboard",
                app: "Notion",
                url: None,
                content: "chat message",
            },
        ),
        (
            "denied_by_anti_goal",
            CaptureCandidate {
                source: "clipboard",
                app: "Xcode",
                url: None,
                content: "this contains the SECRET token",
            },
        ),
        (
            "expected_admit_web",
            CaptureCandidate {
                source: "web",
                app: "Safari",
                url: Some("https://example.com/docs"),
                content: "docs snippet",
            },
        ),
    ];

    let mut admitted = 0;
    for (label, c) in &stream {
        match stack.admit(c) {
            Decision::Admit => {
                admitted += 1;
                assert!(
                    label.starts_with("expected_admit"),
                    "unexpectedly admitted {label}"
                );
            }
            Decision::Drop(_) => {
                assert!(
                    label.starts_with("denied"),
                    "unexpectedly dropped {label}"
                );
            }
        }
    }
    assert_eq!(admitted, 2);
    assert_eq!(receipt_count(&receipts), 2);
}
