//! JSON, JUnit and self-contained HTML evidence.
use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Status {
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
#[derive(Debug, Serialize)]
pub struct Report {
    pub schema: u32,
    pub environment: BTreeMap<String, String>,
    pub firmware: serde_json::Value,
    pub cases: Vec<CaseResult>,
}
impl Report {
    pub fn overall_pass(&self) -> bool {
        !self.cases.iter().any(|c| {
            c.status == Status::Fail
                || (c.tags.iter().any(|t| t == "release") && c.status == Status::Blocked)
        })
    }
    pub fn exit_success(&self) -> bool {
        self.overall_pass()
            && self.cases.iter().all(|c| {
                c.manual
                    || c.status == Status::Pass
                    || (c.status == Status::Skipped && c.message == "filtered by --only")
            })
    }
    pub fn write(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("report.json"), serde_json::to_vec_pretty(self)?)?;
        let failures = self
            .cases
            .iter()
            .filter(|c| c.status == Status::Fail)
            .count();
        let skipped = self
            .cases
            .iter()
            .filter(|c| matches!(c.status, Status::Skipped | Status::Blocked))
            .count();
        let mut xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><testsuite name=\"foxglove-lab\" tests=\"{}\" failures=\"{failures}\" skipped=\"{skipped}\">", self.cases.len());
        let overall = if self.overall_pass() { "PASS" } else { "FAIL" };
        let counts = [Status::Pass, Status::Fail, Status::Blocked, Status::Skipped]
            .map(|s| {
                format!(
                    "<th>{s}</th><td>{}</td>",
                    self.cases.iter().filter(|c| c.status == s).count()
                )
            })
            .join("");
        let mut html = format!("<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Foxglove lab</title><style>:root{{color-scheme:light dark}}body{{font:16px system-ui;max-width:1000px;margin:2em auto;padding:1em}}table{{border-collapse:collapse;width:100%}}td,th{{border:1px solid #888;padding:.5em;text-align:left}}pre{{white-space:pre-wrap;overflow-wrap:anywhere}}img{{max-width:100%;image-rendering:pixelated}}article{{border-top:2px solid #888;margin-top:2em}}</style><h1>Foxglove lab: {overall}</h1><table><tr>{counts}</tr></table><h2>Environment and firmware</h2><pre>{}</pre>", escape(&format!("{:#?}\n{}",self.environment,self.firmware)));
        html.push_str(
            "<h2>Cases</h2><table><tr><th>Case</th><th>Status</th><th>Time (ms)</th></tr>",
        );
        for c in &self.cases {
            html.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape(&c.name),
                c.status,
                c.elapsed_ms
            ));
        }
        html.push_str("</table>");
        for c in &self.cases {
            xml.push_str(&format!(
                "<testcase name=\"{}\" time=\"{:.3}\">",
                escape(&c.name),
                c.elapsed_ms as f64 / 1000.
            ));
            let details = format!(
                "{}\n{}",
                c.message,
                c.steps
                    .iter()
                    .map(|s| format!("{} expected={} actual={}", s.status, s.expected, s.actual))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            match c.status {
                Status::Fail => xml.push_str(&format!(
                    "<failure message=\"{}\">{}</failure>",
                    escape(&c.message),
                    escape(&details)
                )),
                Status::Blocked | Status::Skipped => {
                    xml.push_str(&format!("<skipped message=\"{}\"/>", escape(&details)))
                }
                Status::Pass => {}
            }
            xml.push_str(&format!(
                "<system-out>{}</system-out></testcase>",
                escape(&details)
            ));
            html.push_str(&format!(
                "<article><h2>{}: {}</h2><p>{}</p><p>Manual: {}</p><pre>{}</pre>",
                escape(&c.name),
                c.status,
                escape(&c.description),
                c.manual,
                escape(&c.message)
            ));
            for s in &c.steps {
                html.push_str(&format!(
                    "<h3>{} ({} ms)</h3><pre>Expected: {}\nActual: {}</pre>",
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
        xml.push_str("</testsuite>");
        html.push_str("</html>");
        std::fs::write(dir.join("junit.xml"), xml)?;
        std::fs::write(dir.join("report.html"), html)?;
        Ok(())
    }
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
