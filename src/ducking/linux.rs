//! Linux ducking backend — PulseAudio and PipeWire (through `pipewire-pulse`).
//!
//! Both sound servers expose every application's playback stream as a
//! "sink input" with its own per-channel volume, and both are driven by the
//! same `pactl` CLI, so we shell out to it instead of linking a client
//! library:
//!
//! * `pactl info` — probe: is there a sound server at all?
//! * `pactl -f json list sink-inputs` — enumerate streams (pactl ≥ 16);
//!   falls back to the plain-text `pactl list sink-inputs` on older builds.
//! * `pactl set-sink-input-volume <index> <ch1> <ch2> …` — set one stream's
//!   per-channel volume in raw units (65536 = 100 %).
//!
//! Only sink inputs are touched; the sink (master) volume is never changed.
//! Our own stream (matched by `application.process.id`), corked/paused
//! streams and muted streams are left alone.
//!
//! Fades are done by re-setting each stream's volume every [`RAMP_STEP`]
//! along [`ramp_steps`]; the per-step `pactl` processes for all streams are
//! spawned concurrently and then reaped, so a step costs roughly one process
//! round-trip regardless of how many apps are playing.
//!
//! The parsing code lives in [`parse`] and is free of Linux-specific calls so
//! it can be unit-tested on any host.

use std::io;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Instant;

use super::{
    Ducker, DuckingSettings, PROBE_HOLD, PROBE_SETTINGS, ProbeResult, RAMP_STEP, ramp_steps,
};

pub const BACKEND_NAME: &str = "PulseAudio / PipeWire via pactl";

/// Ducking self-test: check for a sound server, list sink inputs and, if a
/// foreign stream is playing, run a full duck / hold / restore cycle on it.
/// Never touches the failure cooldown.
pub fn probe() -> ProbeResult {
    let info = pactl().arg("info").stdin(Stdio::null()).output();
    match info {
        Err(e) => {
            return ProbeResult::Unsupported(format!(
                "pactl is not available ({e}); install pulseaudio-utils or pipewire-pulse"
            ));
        }
        Ok(out) if !out.status.success() => {
            return ProbeResult::Unsupported(format!(
                "no PulseAudio/PipeWire server: `pactl info` failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(_) => {}
    }
    let inputs = match list_sink_inputs() {
        Ok(inputs) => inputs,
        Err(e) => return ProbeResult::Failed(format!("{e:#}")),
    };
    if parse::select_targets(inputs, std::process::id()).is_empty() {
        return ProbeResult::NothingPlaying;
    }
    let mut ducker = PactlDucker {
        level: PROBE_SETTINGS.level,
        fade_ms: PROBE_SETTINGS.fade_ms,
        streams: Vec::new(),
        ducked: false,
    };
    if let Err(e) = ducker.duck() {
        let _ = ducker.restore();
        return ProbeResult::Failed(format!("{e:#}"));
    }
    if ducker.streams.is_empty() {
        // The stream ended between the two listings.
        return ProbeResult::NothingPlaying;
    }
    thread::sleep(PROBE_HOLD);
    match ducker.restore() {
        Ok(()) => ProbeResult::Ok,
        Err(e) => ProbeResult::Failed(format!("restore failed: {e:#}")),
    }
}

/// Probe for a PulseAudio-compatible server and build the backend.
///
/// Returns `None` when `pactl` is not on `PATH` or `pactl info` fails (no
/// sound server running / no session bus), so the caller plays undocked.
pub fn create(settings: &DuckingSettings) -> Option<Box<dyn Ducker>> {
    let out = match pactl().arg("info").stdin(Stdio::null()).output() {
        Ok(out) => out,
        Err(e) => {
            debug(&format!("pactl not usable ({e}); ducking disabled"));
            return None;
        }
    };
    if !out.status.success() {
        debug(&format!(
            "`pactl info` failed ({}); no sound server, ducking disabled",
            out.status
        ));
        return None;
    }
    if debug_enabled() {
        let info = String::from_utf8_lossy(&out.stdout);
        let server = info
            .lines()
            .find_map(|l| l.strip_prefix("Server Name:"))
            .map(str::trim)
            .unwrap_or("unknown");
        debug(&format!("using pactl backend, server: {server}"));
    }
    Some(Box::new(PactlDucker {
        level: settings.level,
        fade_ms: settings.fade_ms,
        streams: Vec::new(),
        ducked: false,
    }))
}

/// A sink input we are ducking, with the volumes to restore it to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stream {
    index: u32,
    /// Original per-channel raw volumes, in channel-map order.
    volumes: Vec<u32>,
}

struct PactlDucker {
    level: f32,
    fade_ms: u64,
    streams: Vec<Stream>,
    ducked: bool,
}

impl Ducker for PactlDucker {
    fn duck(&mut self) -> anyhow::Result<()> {
        if self.ducked {
            return Ok(());
        }
        let inputs = list_sink_inputs()?;
        self.streams = parse::select_targets(inputs, std::process::id());
        // Flag before ramping so a partially applied fade is still undone by
        // `restore` / `Drop` if something goes wrong midway.
        self.ducked = true;
        if self.streams.is_empty() {
            debug("no other playback streams; nothing to duck");
            return Ok(());
        }
        debug(&format!(
            "ducking {} stream(s) to {:.0}% over {} ms: {:?}",
            self.streams.len(),
            self.level * 100.0,
            self.fade_ms,
            self.streams.iter().map(|s| s.index).collect::<Vec<_>>()
        ));
        ramp(&self.streams, &ramp_steps(1.0, self.level, self.fade_ms));
        Ok(())
    }

    fn restore(&mut self) -> anyhow::Result<()> {
        if !self.ducked {
            return Ok(());
        }
        self.ducked = false;
        let streams = std::mem::take(&mut self.streams);
        if streams.is_empty() {
            return Ok(());
        }
        debug(&format!("restoring {} stream(s)", streams.len()));
        let mut steps = ramp_steps(self.level, 1.0, self.fade_ms);
        // The final step is applied separately with the exact original values.
        steps.pop();
        ramp(&streams, &steps);
        let children: Vec<_> = streams
            .iter()
            .map(|s| spawn_set_volume(s.index, &s.volumes))
            .collect();
        reap(children);
        Ok(())
    }
}

impl Drop for PactlDucker {
    fn drop(&mut self) {
        if let Err(e) = self.restore() {
            eprintln!("[local-voice] ducking: failed to restore volumes: {e:#}");
        }
    }
}

/// Apply every gain in `gains` to all `streams`, one every [`RAMP_STEP`].
/// The spawn/wait time of each step is deducted from its sleep so the fade
/// takes about `fade_ms` even on a slow machine. There is no sleep after the
/// last step.
fn ramp(streams: &[Stream], gains: &[f32]) {
    for (i, &gain) in gains.iter().enumerate() {
        let started = Instant::now();
        let children: Vec<_> = streams
            .iter()
            .map(|s| spawn_set_volume(s.index, &parse::scaled_volumes(&s.volumes, gain)))
            .collect();
        reap(children);
        if i + 1 < gains.len() {
            thread::sleep(RAMP_STEP.saturating_sub(started.elapsed()));
        }
    }
}

/// Spawn `pactl set-sink-input-volume <index> <v1> <v2> …` without waiting.
fn spawn_set_volume(index: u32, volumes: &[u32]) -> io::Result<Child> {
    pactl()
        .args(parse::set_volume_args(index, volumes))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// Wait for a batch of `pactl` children. Failures are ignored on purpose: a
/// stream that ended mid-fade simply makes `pactl` return non-zero.
fn reap(children: Vec<io::Result<Child>>) {
    for child in children {
        match child {
            Ok(mut c) => {
                let _ = c.wait();
            }
            Err(e) => debug(&format!("failed to spawn pactl: {e}")),
        }
    }
}

/// Enumerate sink inputs, preferring the JSON output of pactl ≥ 16 and
/// falling back to the plain-text listing on older releases.
fn list_sink_inputs() -> anyhow::Result<Vec<parse::SinkInput>> {
    let json = pactl()
        .args(["-f", "json", "list", "sink-inputs"])
        .stdin(Stdio::null())
        .output();
    if let Ok(out) = &json
        && out.status.success()
    {
        match parse::parse_json(&String::from_utf8_lossy(&out.stdout)) {
            Ok(inputs) => return Ok(inputs),
            Err(e) => debug(&format!("json listing unparsable ({e}); trying text")),
        }
    } else {
        debug("`pactl -f json` unavailable; falling back to text listing");
    }
    let out = pactl()
        .args(["list", "sink-inputs"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| anyhow::anyhow!("failed to run pactl: {e}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "`pactl list sink-inputs` failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(parse::parse_text(&String::from_utf8_lossy(&out.stdout)))
}

/// A `pactl` command with a fixed C locale so its text output is not
/// translated (the text parser matches English labels).
fn pactl() -> Command {
    let mut cmd = Command::new("pactl");
    cmd.env("LC_ALL", "C").env("LANGUAGE", "C");
    cmd
}

fn debug_enabled() -> bool {
    std::env::var_os("LOCAL_VOICE_DEBUG").is_some_and(|v| v == "1")
}

fn debug(msg: &str) {
    if debug_enabled() {
        eprintln!("[local-voice] ducking: {msg}");
    }
}

/// Pure parsing / arithmetic helpers, kept free of process spawning so they
/// are testable on any host.
mod parse {
    use super::Stream;
    use serde_json::Value;

    /// Raw volume of a channel playing at 100 % (`PA_VOLUME_NORM`).
    pub const VOLUME_NORM: u32 = 65536;

    /// One entry of `pactl list sink-inputs`.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct SinkInput {
        pub index: u32,
        /// `application.process.id` property, when the client reported one.
        pub pid: Option<u32>,
        pub corked: bool,
        pub mute: bool,
        /// Per-channel raw volumes (65536 = 100 %) in channel-map order.
        pub volumes: Vec<u32>,
    }

    /// Choose the streams to duck: everything that is audibly playing and not
    /// ours.
    pub fn select_targets(inputs: Vec<SinkInput>, my_pid: u32) -> Vec<Stream> {
        inputs
            .into_iter()
            .filter(|i| !i.corked && !i.mute && i.pid != Some(my_pid) && !i.volumes.is_empty())
            .map(|i| Stream {
                index: i.index,
                volumes: i.volumes,
            })
            .collect()
    }

    /// `volumes * gain`, rounded to the nearest raw unit.
    pub fn scaled_volumes(volumes: &[u32], gain: f32) -> Vec<u32> {
        let gain = f64::from(gain.clamp(0.0, 1.0));
        volumes
            .iter()
            .map(|&v| (f64::from(v) * gain).round() as u32)
            .collect()
    }

    /// Arguments for `pactl set-sink-input-volume`.
    pub fn set_volume_args(index: u32, volumes: &[u32]) -> Vec<String> {
        let mut args = Vec::with_capacity(volumes.len() + 2);
        args.push("set-sink-input-volume".to_owned());
        args.push(index.to_string());
        args.extend(volumes.iter().map(u32::to_string));
        args
    }

    // ── JSON (`pactl -f json list sink-inputs`, pactl ≥ 16) ──────────────

    pub fn parse_json(s: &str) -> anyhow::Result<Vec<SinkInput>> {
        let root: Value = serde_json::from_str(s)?;
        let items = root
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("expected a JSON array of sink inputs"))?;
        Ok(items.iter().filter_map(json_sink_input).collect())
    }

    fn json_sink_input(v: &Value) -> Option<SinkInput> {
        let index = json_u32(v.get("index")?)?;
        let pid = v
            .get("properties")
            .and_then(|p| p.get("application.process.id"))
            .and_then(json_u32);
        let corked = v.get("corked").map(json_bool).unwrap_or(false);
        let mute = v.get("mute").map(json_bool).unwrap_or(false);
        let volumes = v.get("volume").map(json_volumes).unwrap_or_default();
        // The channel map tells us the order in which `set-sink-input-volume`
        // expects the values; the `volume` object may be re-sorted by the
        // JSON parser.
        let volumes = match v.get("channel_map").and_then(Value::as_str) {
            Some(map) if !map.trim().is_empty() => {
                let ordered: Option<Vec<u32>> = map
                    .split(',')
                    .map(str::trim)
                    .map(|ch| volumes.iter().find(|(n, _)| n == ch).map(|(_, v)| *v))
                    .collect();
                ordered.unwrap_or_else(|| volumes.iter().map(|(_, v)| *v).collect())
            }
            _ => volumes.into_iter().map(|(_, v)| v).collect(),
        };
        Some(SinkInput {
            index,
            pid,
            corked,
            mute,
            volumes,
        })
    }

    /// `(channel name, raw value)` pairs from the `volume` object.
    fn json_volumes(v: &Value) -> Vec<(String, u32)> {
        let Some(obj) = v.as_object() else {
            return Vec::new();
        };
        obj.iter()
            .filter_map(|(name, ch)| {
                let raw = ch
                    .get("value")
                    .and_then(json_u32)
                    .or_else(|| ch.get("value_percent").and_then(json_percent))?;
                Some((name.clone(), raw))
            })
            .collect()
    }

    fn json_u32(v: &Value) -> Option<u32> {
        match v {
            Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    fn json_percent(v: &Value) -> Option<u32> {
        v.as_str().and_then(percent_to_raw)
    }

    fn json_bool(v: &Value) -> bool {
        match v {
            Value::Bool(b) => *b,
            Value::String(s) => matches!(s.trim(), "yes" | "true" | "1"),
            Value::Number(n) => n.as_u64().is_some_and(|n| n != 0),
            _ => false,
        }
    }

    // ── Text (`pactl list sink-inputs`) ──────────────────────────────────

    pub fn parse_text(s: &str) -> Vec<SinkInput> {
        let mut out = Vec::new();
        let mut cur: Option<SinkInput> = None;
        for line in s.lines() {
            let t = line.trim();
            if let Some(idx) = t.strip_prefix("Sink Input #") {
                if let Some(done) = cur.take() {
                    out.push(done);
                }
                cur = idx.trim().parse().ok().map(|index| SinkInput {
                    index,
                    pid: None,
                    corked: false,
                    mute: false,
                    volumes: Vec::new(),
                });
                continue;
            }
            let Some(si) = cur.as_mut() else {
                continue;
            };
            if let Some(v) = t.strip_prefix("Corked:") {
                si.corked = text_bool(v);
            } else if let Some(v) = t.strip_prefix("Mute:") {
                si.mute = text_bool(v);
            } else if let Some(v) = t.strip_prefix("Volume:") {
                si.volumes = parse_volume_line(v);
            } else if let Some(v) = t.strip_prefix("application.process.id") {
                let v = v.trim_start().strip_prefix('=').unwrap_or(v);
                si.pid = v.trim().trim_matches('"').parse().ok();
            }
        }
        if let Some(done) = cur {
            out.push(done);
        }
        out
    }

    fn text_bool(v: &str) -> bool {
        matches!(v.trim(), "yes" | "true" | "1")
    }

    /// Parse the part after `Volume:`. Modern pactl prints
    /// `front-left: 65536 / 100% / 0.00 dB,   front-right: 65536 / 100% / …`;
    /// very old releases print `0:  80% 1:  80%` (no raw value), which is
    /// converted from the percentage.
    fn parse_volume_line(v: &str) -> Vec<u32> {
        let v = v.trim();
        if v.contains('/') {
            v.split(',')
                .filter_map(|ch| {
                    let (_, rest) = ch.split_once(':')?;
                    let first = rest.split('/').next()?.trim();
                    first.parse().ok().or_else(|| percent_to_raw(first))
                })
                .collect()
        } else {
            // "0:  80% 1:  80%" — every token ending in '%' is a channel.
            v.split_whitespace()
                .filter(|tok| tok.ends_with('%'))
                .filter_map(percent_to_raw)
                .collect()
        }
    }

    fn percent_to_raw(s: &str) -> Option<u32> {
        let pct: f64 = s.trim().strip_suffix('%')?.trim().parse().ok()?;
        Some((pct / 100.0 * f64::from(VOLUME_NORM)).round() as u32)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Trimmed from real `pactl -f json list sink-inputs` output
        /// (pactl 16.1 on pipewire-pulse). Stream 7 is ours, 9 is corked,
        /// 11 is muted, 12 is 5.1 with a channel map that a sorted JSON
        /// object would scramble.
        const JSON: &str = r#"[
  {
    "index": 5,
    "driver": "protocol-native.c",
    "owner_module": 11,
    "client": 86,
    "sink": 1,
    "sample_specification": "float32le 2ch 48000Hz",
    "channel_map": "front-left,front-right",
    "format": "pcm, format.sample_format = \"\\\"float32le\\\"\"  format.rate = \"48000\"  format.channels = \"2\"  format.channel_map = \"\\\"front-left,front-right\\\"\"",
    "corked": false,
    "mute": false,
    "volume": {
      "front-left": { "value": 52428, "value_percent": "80%", "db": "-5.81 dB" },
      "front-right": { "value": 65536, "value_percent": "100%", "db": "0.00 dB" }
    },
    "balance": 0.20,
    "buffer_latency_usec": 0,
    "sink_latency_usec": 0,
    "resample_method": "PipeWire",
    "properties": {
      "application.name": "Firefox",
      "application.process.id": "4242",
      "media.role": "music"
    }
  },
  {
    "index": 7,
    "channel_map": "front-left,front-right",
    "corked": false,
    "mute": false,
    "volume": {
      "front-left": { "value": 65536, "value_percent": "100%", "db": "0.00 dB" },
      "front-right": { "value": 65536, "value_percent": "100%", "db": "0.00 dB" }
    },
    "properties": { "application.name": "local-voice", "application.process.id": "1000" }
  },
  {
    "index": 9,
    "channel_map": "mono",
    "corked": true,
    "mute": false,
    "volume": { "mono": { "value": 65536, "value_percent": "100%", "db": "0.00 dB" } },
    "properties": { "application.process.id": "31" }
  },
  {
    "index": 11,
    "channel_map": "front-left,front-right",
    "corked": false,
    "mute": true,
    "volume": {
      "front-left": { "value": 65536, "value_percent": "100%", "db": "0.00 dB" },
      "front-right": { "value": 65536, "value_percent": "100%", "db": "0.00 dB" }
    },
    "properties": { "application.process.id": "32" }
  },
  {
    "index": 12,
    "channel_map": "front-left,front-right,front-center,lfe,rear-left,rear-right",
    "corked": false,
    "mute": false,
    "volume": {
      "front-left": { "value": 1, "value_percent": "0%", "db": "-inf dB" },
      "front-right": { "value": 2, "value_percent": "0%", "db": "-inf dB" },
      "front-center": { "value": 3, "value_percent": "0%", "db": "-inf dB" },
      "lfe": { "value": 4, "value_percent": "0%", "db": "-inf dB" },
      "rear-left": { "value": 5, "value_percent": "0%", "db": "-inf dB" },
      "rear-right": { "value": 6, "value_percent": "0%", "db": "-inf dB" }
    },
    "properties": {}
  }
]"#;

        /// Trimmed from real `pactl list sink-inputs` (LC_ALL=C) output.
        const TEXT: &str = "Sink Input #5
\tDriver: protocol-native.c
\tOwner Module: 11
\tClient: 86
\tSink: 1
\tSample Specification: float32le 2ch 48000Hz
\tChannel Map: front-left,front-right
\tFormat: pcm, format.sample_format = \"\\\"float32le\\\"\"  format.rate = \"48000\"
\tCorked: no
\tMute: no
\tVolume: front-left: 52428 /  80% / -5.81 dB,   front-right: 65536 / 100% / 0.00 dB
\t        balance 0.20
\tBuffer Latency: 0 usec
\tSink Latency: 0 usec
\tResample method: PipeWire
\tProperties:
\t\tapplication.name = \"Firefox\"
\t\tapplication.process.id = \"4242\"
\t\tmedia.role = \"music\"

Sink Input #7
\tCorked: no
\tMute: no
\tVolume: front-left: 65536 / 100% / 0.00 dB,   front-right: 65536 / 100% / 0.00 dB
\t        balance 0.00
\tProperties:
\t\tapplication.name = \"local-voice\"
\t\tapplication.process.id = \"1000\"

Sink Input #9
\tCorked: yes
\tMute: no
\tVolume: mono: 65536 / 100% / 0.00 dB
\t        balance 0.00
\tProperties:
\t\tapplication.process.id = \"31\"

Sink Input #11
\tCorked: no
\tMute: yes
\tVolume: front-left: 65536 / 100% / 0.00 dB,   front-right: 65536 / 100% / 0.00 dB
\tProperties:
\t\tapplication.process.id = \"32\"

Sink Input #13
\tCorked: no
\tMute: no
\tVolume: 0:  80% 1:  80%
\tProperties:
\t\tapplication.process.id = \"33\"
";

        fn by_index(inputs: &[SinkInput], index: u32) -> &SinkInput {
            inputs.iter().find(|i| i.index == index).unwrap()
        }

        #[test]
        fn json_parses_fields_and_channel_order() {
            let inputs = parse_json(JSON).unwrap();
            assert_eq!(inputs.len(), 5);
            let s5 = by_index(&inputs, 5);
            assert_eq!(s5.pid, Some(4242));
            assert!(!s5.corked && !s5.mute);
            assert_eq!(s5.volumes, vec![52428, 65536]);
            assert_eq!(by_index(&inputs, 7).pid, Some(1000));
            assert!(by_index(&inputs, 9).corked);
            assert_eq!(by_index(&inputs, 9).volumes, vec![65536]);
            assert!(by_index(&inputs, 11).mute);
            // 5.1 must come out in channel-map order, not key-sorted order.
            let s12 = by_index(&inputs, 12);
            assert_eq!(s12.pid, None);
            assert_eq!(s12.volumes, vec![1, 2, 3, 4, 5, 6]);
        }

        #[test]
        fn json_tolerates_string_values_and_missing_channel_map() {
            let s = r#"[{"index":"3","corked":"no","mute":"yes",
                "volume":{"mono":{"value":"32768","value_percent":"50%"}},
                "properties":{"application.process.id":77}}]"#;
            let inputs = parse_json(s).unwrap();
            assert_eq!(
                inputs,
                vec![SinkInput {
                    index: 3,
                    pid: Some(77),
                    corked: false,
                    mute: true,
                    volumes: vec![32768],
                }]
            );
            // Percent fallback when `value` is absent.
            let s = r#"[{"index":4,"volume":{"mono":{"value_percent":"50%"}}}]"#;
            assert_eq!(parse_json(s).unwrap()[0].volumes, vec![32768]);
        }

        #[test]
        fn json_rejects_garbage() {
            assert!(parse_json("not json").is_err());
            assert!(parse_json(r#"{"index":1}"#).is_err());
            // An entry without an index is dropped, not fatal.
            assert!(parse_json(r#"[{"corked":false}]"#).unwrap().is_empty());
            assert!(parse_json("[]").unwrap().is_empty());
        }

        #[test]
        fn text_parses_fields() {
            let inputs = parse_text(TEXT);
            assert_eq!(inputs.len(), 5);
            let s5 = by_index(&inputs, 5);
            assert_eq!(s5.pid, Some(4242));
            assert!(!s5.corked && !s5.mute);
            assert_eq!(s5.volumes, vec![52428, 65536]);
            assert_eq!(by_index(&inputs, 7).pid, Some(1000));
            assert!(by_index(&inputs, 9).corked);
            assert_eq!(by_index(&inputs, 9).volumes, vec![65536]);
            assert!(by_index(&inputs, 11).mute);
            // Legacy percent-only format.
            assert_eq!(by_index(&inputs, 13).volumes, vec![52429, 52429]);
        }

        #[test]
        fn text_and_json_agree() {
            let json = parse_json(JSON).unwrap();
            let text = parse_text(TEXT);
            for idx in [5, 7, 9, 11] {
                assert_eq!(
                    by_index(&json, idx),
                    by_index(&text, idx),
                    "sink input #{idx}"
                );
            }
        }

        #[test]
        fn text_handles_empty_and_noise() {
            assert!(parse_text("").is_empty());
            assert!(parse_text("Sink Input #x\n\tCorked: no\n").is_empty());
            let one = parse_text("garbage\nSink Input #2\n\tVolume: mono: 100 / 0% / -inf dB\n");
            assert_eq!(one.len(), 1);
            assert_eq!(one[0].volumes, vec![100]);
        }

        #[test]
        fn select_targets_skips_self_corked_muted_and_empty() {
            let mut inputs = parse_json(JSON).unwrap();
            inputs.push(SinkInput {
                index: 99,
                pid: Some(1),
                corked: false,
                mute: false,
                volumes: vec![],
            });
            let targets = select_targets(inputs, 1000);
            assert_eq!(
                targets,
                vec![
                    Stream {
                        index: 5,
                        volumes: vec![52428, 65536]
                    },
                    Stream {
                        index: 12,
                        volumes: vec![1, 2, 3, 4, 5, 6]
                    },
                ]
            );
        }

        #[test]
        fn scaling_rounds_and_round_trips_at_unity() {
            assert_eq!(scaled_volumes(&[65536, 52428], 0.2), vec![13107, 10486]);
            assert_eq!(scaled_volumes(&[65536, 1, 6], 1.0), vec![65536, 1, 6]);
            assert_eq!(scaled_volumes(&[65536], 0.0), vec![0]);
            assert_eq!(scaled_volumes(&[65536], 7.0), vec![65536]);
        }

        #[test]
        fn set_volume_args_are_one_value_per_channel() {
            assert_eq!(
                set_volume_args(42, &[13107, 10486]),
                vec!["set-sink-input-volume", "42", "13107", "10486"]
            );
        }

        #[test]
        fn percent_conversion() {
            assert_eq!(percent_to_raw("100%"), Some(65536));
            assert_eq!(percent_to_raw(" 50% "), Some(32768));
            assert_eq!(percent_to_raw("0%"), Some(0));
            assert_eq!(percent_to_raw("50"), None);
        }
    }
}
