//! Presentation seam: the text of the toast an alert raises.

use uqt_core::engine::{Alert, AlertKind};
use uqt_core::time::Timestamp;
use uqt_ui::present::{AlertMessage, Moment, alert_message, toast_xml};

fn alert(kind: AlertKind, window_end: Option<Timestamp>) -> Alert {
    Alert {
        assistant_id: "t",
        assistant_name: "Test Assistant".into(),
        account_id: "me".into(),
        account_name: "Work".into(),
        meter_key: "weekly".into(),
        meter_label: "Weekly".into(),
        window_end,
        kind,
    }
}

fn moment() -> Moment {
    Moment {
        now: Timestamp::from_utc(2026, 10, 29, 12, 0, 0),
        utc_offset_secs: 3600,
    }
}

fn reset_at() -> Option<Timestamp> {
    Some(Timestamp::from_utc(2026, 10, 29, 14, 5, 0))
}

#[test]
fn a_threshold_alert_names_the_usage_and_when_it_resets() {
    let m = alert_message(
        &alert(
            AlertKind::Threshold {
                percent: 90,
                used_fraction: 0.914,
            },
            reset_at(),
        ),
        moment(),
    );
    assert_eq!(m.title, "Weekly at 91%");
    assert_eq!(
        m.body,
        "Test Assistant \u{b7} Work \u{b7} resets in 2 h 5 min"
    );
}

#[test]
fn the_last_threshold_says_the_limit_is_reached() {
    let m = alert_message(
        &alert(
            AlertKind::Threshold {
                percent: 100,
                used_fraction: 1.02,
            },
            reset_at(),
        ),
        moment(),
    );
    assert_eq!(m.title, "Weekly limit reached");
}

#[test]
fn a_meter_without_a_window_has_no_reset_time() {
    let m = alert_message(
        &alert(
            AlertKind::Threshold {
                percent: 75,
                used_fraction: 0.8,
            },
            None,
        ),
        moment(),
    );
    assert_eq!(m.body, "Test Assistant \u{b7} Work");
}

#[test]
fn the_toast_xml_escapes_the_text_and_opens_the_popup_on_click() {
    let xml = toast_xml(&AlertMessage {
        title: "Spend <monthly> at 90%".into(),
        body: "R&D \u{b7} \"Work\"".into(),
    });
    assert_eq!(
        xml,
        "<toast launch=\"open-popup\"><visual><binding template=\"ToastGeneric\">\
         <text>Spend &lt;monthly&gt; at 90%</text>\
         <text>R&amp;D \u{b7} &quot;Work&quot;</text>\
         </binding></visual></toast>"
    );
}

#[test]
fn a_reset_alert_says_capacity_is_back() {
    let m = alert_message(&alert(AlertKind::Reset, None), moment());
    assert_eq!(m.title, "Weekly has reset");
    assert_eq!(m.body, "Test Assistant \u{b7} Work");
}
