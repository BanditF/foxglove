//! The release-gate report: report.json (schema `foxglove-lab-report`, version 2, described
//! in docs/test-lab/report-format.md and docs/test-lab/report.schema.json), junit.xml (one
//! testsuite per area) and a self-contained report.html whose first screen is the summary.
use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use serde_json::json;
use std::{collections::BTreeMap, path::Path};

pub const SCHEMA: &str = "foxglove-lab-report";
pub const VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Status {
    #[default]
    Pass,
    Fail,
    Blocked,
    Skipped,
}
impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Pass => "PASS",
                Self::Fail => "FAIL",
                Self::Blocked => "BLOCKED",
                Self::Skipped => "SKIPPED",
            }
        )
    }
}
#[derive(Debug, Serialize)]
pub struct StepResult {
    pub status: Status,
    pub elapsed_ms: u128,
    pub expected: serde_json::Value,
    pub actual: String,
    pub attachments: Vec<String>,
}
#[derive(Debug, Serialize)]
pub struct CaseResult {
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub manual: bool,
    pub status: Status,
    pub elapsed_ms: u128,
    pub message: String,
    pub steps: Vec<StepResult>,
}
impl CaseResult {
    /// The area a case is reported under: its first tag that isn't "release" (which marks
    /// gate membership, not a topic), else "general".
    pub fn area(&self) -> &str {
        self.tags
            .iter()
            .map(String::as_str)
            .find(|t| *t != "release")
            .unwrap_or("general")
    }
    fn gating(&self) -> bool {
        self.tags.iter().any(|t| t == "release")
    }
}

/// One half's running build, from D7 09 BUILD_INFO.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Build {
    pub version: String,
    /// "dev" (test hooks) or "ga".
    pub profile: String,
    pub sha: String,
}
#[derive(Debug, Default, Serialize)]
pub struct Firmware {
    /// The decoded HELLO, or null without a connection.
    pub hello: serde_json::Value,
    /// null when the firmware can't tell (no HELLO bit 12) or the right isn't reachable.
    pub left: Option<Build>,
    pub right: Option<Build>,
}
#[derive(Debug, Default, Serialize)]
pub struct App {
    pub running: bool,
    pub path: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub schema: &'static str,
    pub version: u32,
    pub suite: String,
    pub generated_utc: String,
    pub environment: BTreeMap<String, String>,
    pub firmware: Firmware,
    pub app: App,
    pub cases: Vec<CaseResult>,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Area {
    pub name: String,
    pub verdict: Status,
    pub pass: usize,
    pub fail: usize,
    pub blocked: usize,
    pub skipped: usize,
}
impl Report {
    pub fn new(suite: &str, generated_utc: String, environment: BTreeMap<String, String>) -> Self {
        Self {
            schema: SCHEMA,
            version: VERSION,
            suite: suite.into(),
            generated_utc,
            environment,
            firmware: Firmware::default(),
            app: App::default(),
            cases: Vec::new(),
        }
    }
    /// The release gate: no case failed, and no case tagged "release" is blocked.
    pub fn overall_pass(&self) -> bool {
        !self
            .cases
            .iter()
            .any(|c| c.status == Status::Fail || (c.gating() && c.status == Status::Blocked))
    }
    pub fn exit_success(&self) -> bool {
        self.overall_pass()
            && self.cases.iter().all(|c| {
                c.manual
                    || c.status == Status::Pass
                    || (c.status == Status::Skipped && c.message == "filtered by --only")
            })
    }
    /// Per-area counts, in first-seen order. An area is FAIL if any case failed, BLOCKED if a
    /// gating case is blocked, else PASS.
    pub fn areas(&self) -> Vec<Area> {
        let mut areas: Vec<Area> = Vec::new();
        for c in &self.cases {
            let i = match areas.iter().position(|a| a.name == c.area()) {
                Some(i) => i,
                None => {
                    areas.push(Area {
                        name: c.area().into(),
                        ..Area::default()
                    });
                    areas.len() - 1
                }
            };
            let a = &mut areas[i];
            match c.status {
                Status::Pass => a.pass += 1,
                Status::Fail => a.fail += 1,
                Status::Blocked => a.blocked += 1,
                Status::Skipped => a.skipped += 1,
            }
            if c.status == Status::Fail {
                a.verdict = Status::Fail;
            } else if c.status == Status::Blocked && c.gating() && a.verdict != Status::Fail {
                a.verdict = Status::Blocked;
            }
        }
        areas
    }
    /// FAIL if a case failed; BLOCKED if none did but a `release` case is blocked (a manual
    /// step in a non-interactive run, a missing capability); PASS otherwise.
    pub fn verdict(&self) -> Status {
        if self.cases.iter().any(|c| c.status == Status::Fail) {
            Status::Fail
        } else if self.overall_pass() {
            Status::Pass
        } else {
            Status::Blocked
        }
    }
    /// `run`'s exit code: 0 when `exit_success`, 1 when a case failed, 2 when nothing failed
    /// but something couldn't run (the lab scripts' and release-test.ps1's BLOCKED).
    pub fn exit_code(&self) -> u8 {
        if self.exit_success() {
            0
        } else if self.cases.iter().any(|c| c.status == Status::Fail) {
            1
        } else {
            2
        }
    }
    fn json(&self) -> Result<serde_json::Value> {
        let mut v = serde_json::to_value(self)?;
        v["verdict"] = json!(self.verdict());
        v["areas"] = serde_json::to_value(self.areas())?;
        let count = |s| self.cases.iter().filter(|c| c.status == s).count();
        v["counts"] = json!({"pass": count(Status::Pass), "fail": count(Status::Fail),
                             "blocked": count(Status::Blocked), "skipped": count(Status::Skipped)});
        for (i, c) in self.cases.iter().enumerate() {
            v["cases"][i]["area"] = json!(c.area());
            v["cases"][i]["id"] = json!(case_id(i));
        }
        Ok(v)
    }
    pub fn write(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            dir.join("report.json"),
            serde_json::to_vec_pretty(&self.json()?)?,
        )?;
        std::fs::write(dir.join("junit.xml"), self.junit())?;
        std::fs::write(dir.join("report.html"), self.html(dir)?)?;
        Ok(())
    }

    fn build_text(b: &Option<Build>) -> String {
        b.as_ref().map_or("unknown".into(), |b| {
            format!("{} {} {}", b.version, b.profile, b.sha)
        })
    }

    fn junit(&self) -> String {
        let props = format!(
            "<properties><property name=\"suite\" value=\"{}\"/><property name=\"verdict\" value=\"{}\"/><property name=\"firmware.left\" value=\"{}\"/><property name=\"firmware.right\" value=\"{}\"/><property name=\"app.version\" value=\"{}\"/><property name=\"git_sha\" value=\"{}\"/></properties>",
            escape(&self.suite),
            self.verdict(),
            escape(&Self::build_text(&self.firmware.left)),
            escape(&Self::build_text(&self.firmware.right)),
            escape(self.app.version.as_deref().unwrap_or("unknown")),
            escape(self.environment.get("git_sha").map_or("unknown", String::as_str)),
        );
        let total = |f: fn(&Area) -> usize| self.areas().iter().map(f).sum::<usize>();
        let mut xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"foxglove-lab\" tests=\"{}\" failures=\"{}\" skipped=\"{}\">\n",
            self.cases.len(),
            total(|a| a.fail),
            total(|a| a.blocked + a.skipped),
        );
        for area in self.areas() {
            let cases: Vec<_> = self
                .cases
                .iter()
                .filter(|c| c.area() == area.name)
                .collect();
            let time: f64 = cases.iter().map(|c| c.elapsed_ms as f64 / 1000.).sum();
            xml.push_str(&format!(
                "<testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" skipped=\"{}\" time=\"{time:.3}\">{props}\n",
                escape(&area.name),
                cases.len(),
                area.fail,
                area.blocked + area.skipped,
            ));
            for c in cases {
                let details = format!(
                    "{}\n{}",
                    c.message,
                    c.steps
                        .iter()
                        .map(|s| format!(
                            "{} expected={} actual={}",
                            s.status, s.expected, s.actual
                        ))
                        .collect::<Vec<_>>()
                        .join("\n")
                );
                xml.push_str(&format!(
                    "<testcase classname=\"foxglove-lab.{}\" name=\"{}\" time=\"{:.3}\">",
                    escape(&area.name),
                    escape(&c.name),
                    c.elapsed_ms as f64 / 1000.
                ));
                match c.status {
                    Status::Fail => xml.push_str(&format!(
                        "<failure message=\"{}\">{}</failure>",
                        escape(&c.message),
                        escape(&details)
                    )),
                    Status::Blocked | Status::Skipped => xml.push_str(&format!(
                        "<skipped message=\"{}: {}\"/>",
                        c.status,
                        escape(&c.message)
                    )),
                    Status::Pass => {}
                }
                xml.push_str(&format!(
                    "<system-out>{}</system-out></testcase>\n",
                    escape(&details)
                ));
            }
            xml.push_str("</testsuite>\n");
        }
        xml.push_str("</testsuites>\n");
        xml
    }

    fn html(&self, dir: &Path) -> Result<String> {
        let verdict = self.verdict();
        let env = |k: &str| escape(self.environment.get(k).map_or("unknown", String::as_str));
        let mut html = format!(
            "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Foxglove release gate</title><style>:root{{color-scheme:light dark}}body{{font:16px system-ui;max-width:1000px;margin:2em auto;padding:1em}}table{{border-collapse:collapse;width:100%;margin-bottom:1em}}td,th{{border:1px solid #888;padding:.4em .6em;text-align:left}}pre{{white-space:pre-wrap;overflow-wrap:anywhere}}img{{max-width:100%;image-rendering:pixelated}}article{{border-top:2px solid #888;margin-top:2em}}.PASS{{color:#1a7f37}}.FAIL{{color:#cf222e}}.BLOCKED{{color:#9a6700}}.SKIPPED{{color:#6e7781}}</style>\
             <h1>Foxglove release gate: <span class=\"{verdict}\">{verdict}</span></h1>\
             <p>Suite <b>{}</b> · {} · repo {} · runner {}</p>",
            escape(&self.suite),
            escape(&self.generated_utc),
            env("git_sha"),
            env("runner_version"),
        );
        html.push_str("<h2>Areas</h2><table><tr><th>Area</th><th>Verdict</th><th>Pass</th><th>Fail</th><th>Blocked</th><th>Skipped</th></tr>");
        for a in self.areas() {
            html.push_str(&format!(
                "<tr><td>{}</td><td class=\"{v}\">{v}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape(&a.name),
                a.pass,
                a.fail,
                a.blocked,
                a.skipped,
                v = a.verdict
            ));
        }
        html.push_str("</table><h2>Firmware and app</h2><table><tr><th></th><th>Version</th><th>Profile</th><th>SHA</th></tr>");
        for (half, b) in [
            ("Left", &self.firmware.left),
            ("Right", &self.firmware.right),
        ] {
            let b = b.clone().unwrap_or_else(|| Build {
                version: "unknown".into(),
                ..Build::default()
            });
            html.push_str(&format!(
                "<tr><td>{half}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape(&b.version),
                escape(&b.profile),
                escape(&b.sha)
            ));
        }
        html.push_str(&format!(
            "<tr><td>App</td><td colspan=\"3\">{} {}</td></tr></table>",
            escape(self.app.version.as_deref().unwrap_or(if self.app.running {
                "running, version unknown"
            } else {
                "not running"
            })),
            escape(self.app.path.as_deref().unwrap_or("")),
        ));
        html.push_str("<h2>Cases</h2><table><tr><th>Case</th><th>Area</th><th>Status</th><th>Captures</th><th>Time (ms)</th></tr>");
        for (i, c) in self.cases.iter().enumerate() {
            let captures: Vec<_> = c.steps.iter().flat_map(|s| s.attachments.iter()).collect();
            html.push_str(&format!(
                "<tr><td><a href=\"#{id}\">{}</a></td><td>{}</td><td class=\"{s}\">{s}</td><td>{}</td><td>{}</td></tr>",
                escape(&c.name),
                escape(c.area()),
                captures
                    .iter()
                    .map(|a| format!("<a href=\"{0}\">{0}</a>", escape(a)))
                    .collect::<Vec<_>>()
                    .join(" "),
                c.elapsed_ms,
                id = case_id(i),
                s = c.status,
            ));
        }
        html.push_str(&format!(
            "</table><details><summary>Environment and HELLO</summary><pre>{}</pre></details>",
            escape(&format!(
                "{:#?}\n{}",
                self.environment,
                serde_json::to_string_pretty(&self.firmware.hello)?
            ))
        ));
        for (i, c) in self.cases.iter().enumerate() {
            html.push_str(&format!(
                "<article id=\"{}\"><h2>{}: <span class=\"{s}\">{s}</span></h2><p>{}</p><p>Area: {} · Manual: {}</p><pre>{}</pre>",
                case_id(i),
                escape(&c.name),
                escape(&c.description),
                escape(c.area()),
                c.manual,
                escape(&c.message),
                s = c.status,
            ));
            for s in &c.steps {
                html.push_str(&format!(
                    "<h3 class=\"{}\">{} ({} ms)</h3><pre>Expected: {}\nActual: {}</pre>",
                    s.status,
                    s.status,
                    s.elapsed_ms,
                    escape(&s.expected.to_string()),
                    escape(&s.actual)
                ));
                for attachment in &s.attachments {
                    let bytes = std::fs::read(dir.join(attachment))?;
                    html.push_str(&format!(
                        "<p>{}</p><img alt=\"{}\" src=\"data:image/png;base64,{}\">",
                        escape(attachment),
                        escape(attachment),
                        STANDARD.encode(bytes)
                    ));
                }
            }
            html.push_str("</article>");
        }
        html.push_str("</html>");
        Ok(html)
    }
}
fn case_id(i: usize) -> String {
    format!("case-{}", i + 1)
}
fn escape(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
