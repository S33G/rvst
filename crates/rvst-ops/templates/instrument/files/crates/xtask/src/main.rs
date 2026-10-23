use rvst_core::{EventSink, Plugin};
use std::{env, path::Path};
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
fn value(args: &[String], key: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Some(i) = args.iter().position(|v| v == key) else {
        return Ok(None);
    };
    Ok(Some(
        args.get(i + 1)
            .filter(|v| !v.starts_with("--"))
            .ok_or_else(|| format!("{key} needs a value"))?
            .clone(),
    ))
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let release = args.iter().any(|v| v == "--release");
    match args.first().map(String::as_str){
 Some("package"|"bundle"|"install")=>{
  let install=args[0]=="install";let format=args.get(1).filter(|v|!v.starts_with('-')).map(String::as_str).unwrap_or("all");if !matches!(format,"all"|"vst3"|"clap"){return Err("Expected all, vst3, or clap".into());}
  let mut features:Vec<String>=value(&args,"--features")?.unwrap_or_default().split(',').filter(|s|!s.is_empty()).map(String::from).collect();if !release&&value(&args,"--profile")?.as_deref().is_none_or(|p|p=="dev"||p=="test")&&!features.iter().any(|s|s=="devtools"){features.push("devtools".into());}
  let options=rvst_build::BundleOptions{release,target:value(&args,"--target")?,profile:value(&args,"--profile")?,features};let package=value(&args,"--package")?.unwrap_or_else(||"plugin-native".into());
  for (name,kind) in [("vst3",rvst_build::Format::Vst3),("clap",rvst_build::Format::Clap)]{if format=="all"||format==name{let bundle=rvst_build::bundle_with_options(&root,&package,kind,&options)?;println!("{}",if install{rvst_build::install(&bundle)?}else{bundle}.display());}}
 }
 Some("web")=>{rvst_build::web::run(&root,&rvst_build::web::WebOptions{package:value(&args,"--package")?.unwrap_or_else(||"plugin-web".into()),release,target:value(&args,"--target")?,profile:value(&args,"--profile")?,features:value(&args,"--features")?.unwrap_or_default().split(',').filter(|s|!s.is_empty()).map(String::from).collect(),serve:args.iter().any(|v|v=="--serve"),open:args.iter().any(|v|v=="--open"),port:value(&args,"--port")?.map(|v|v.parse()).transpose()?.unwrap_or(8787)})?;}
 Some("debug")=>{println!("{}",serde_json::to_string_pretty(plugin_core::DemoPlugin::descriptor())?);println!("Native debug: lldb -- target/debug/plugin-standalone\nAttach to a DAW: lldb -p <PID>\nLinux: gdb --args target/debug/plugin-standalone\nBrowser logs: cargo rvst inspect --open\nVS Code: choose the supplied CodeLLDB launch/attach configuration.");}
 Some("record")=>{let path=args.get(1).ok_or("record needs an output path")?;let blocks:usize=value(&args,"--blocks")?.map(|v|v.parse()).transpose()?.unwrap_or(64);if blocks==0||blocks>10000{return Err("--blocks must be 1..10000".into());}let recording=record(blocks)?;use std::io::Write;let mut file=std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;file.write_all(&serde_json::to_vec_pretty(&recording)?)?;println!("Recorded {blocks} deterministic blocks to {path}");}
 Some("replay")=>{let path=args.get(1).ok_or("replay needs a recording JSON path")?;let recording:rvst_runtime::Recording=serde_json::from_slice(&std::fs::read(path)?)?;if args.iter().any(|v|v=="--step"){let mut cursor=recording.cursor::<plugin_core::DemoPlugin>(1e-6)?;while let Some(block)=cursor.step()?{println!("{}",serde_json::to_string(&block)?);}}let report=recording.replay::<plugin_core::DemoPlugin>(1e-6)?;println!("{}",serde_json::to_string_pretty(&report)?);if !report.matches{return Err("Replay output differs from the capture".into());}}
 _=>return Err("Usage: cargo xtask <bundle|install> [all|vst3|clap] [--release] [--target T] [--profile P] [--features F] | web --serve [--open] | debug | record PATH [--blocks N] | replay PATH [--step]".into()),
 }
    Ok(())
}
struct Events {
    values: Vec<rvst_core::Event>,
    overflow: bool,
}
impl EventSink for Events {
    fn push(&mut self, event: rvst_core::Event) -> bool {
        if self.values.len() == self.values.capacity() {
            self.overflow = true;
            false
        } else {
            self.values.push(event);
            true
        }
    }
}
fn record(blocks: usize) -> Result<rvst_runtime::Recording, Box<dyn std::error::Error>> {
    use rvst_core::*;
    use rvst_runtime::*;
    let descriptor = plugin_core::DemoPlugin::descriptor();
    let config = ProcessConfig {
        sample_rate: 48000.,
        max_frames: 128,
        input_channels: descriptor.input_channels(),
        output_channels: descriptor.output_channels(),
    };
    let mut plugin = plugin_core::DemoPlugin::new();
    plugin.prepare(config)?;
    plugin.reset();
    let initial_state = plugin.save_state()?;
    let (mut recorder, mut drain) = recording_channel(
        2,
        128 * config.input_channels.max(config.output_channels),
        4096,
    );
    let mut recorded = Vec::with_capacity(blocks);
    let mut output_events = Events {
        values: Vec::with_capacity(4096),
        overflow: false,
    };
    let mut sample_clock = 0usize;
    for index in 0..blocks {
        let frames = if index % 2 == 0 { 128 } else { 64 };
        let input: Vec<f32> = (0..frames * config.input_channels)
            .map(|i| ((sample_clock + i % frames) as f32 * 0.04).sin() * 0.05)
            .collect();
        let mut output = vec![0.; frames * config.output_channels];
        let mut events = Vec::new();
        if descriptor.midi_input && index == 0 {
            events.push(Event {
                frame: 0,
                kind: EventKind::NoteOn {
                    channel: 0,
                    key: 60,
                    velocity: 0.5,
                    note_id: 1,
                },
            });
        }
        if descriptor.midi_input && index == blocks - 1 {
            events.push(Event {
                frame: (frames - 1) as u32,
                kind: EventKind::NoteOff {
                    channel: 0,
                    key: 60,
                    velocity: 0.,
                    note_id: 1,
                },
            });
        }
        let transport = Transport {
            playing: true,
            sample_position: Some(sample_clock as i64),
            tempo: Some(120.),
            ..Default::default()
        };
        output_events.values.clear();
        output_events.overflow = false;
        let mut audio = AudioBlock::new(
            &input,
            &mut output,
            frames,
            config.input_channels,
            config.output_channels,
        )?;
        let status = plugin.process(&mut audio, &events, &transport, &mut output_events);
        if output_events.overflow {
            recorder.mark_incomplete();
        }
        recorder.capture_block_f32(&audio, &events, transport, &output_events.values, status);
        if let Some(block) = drain.pop() {
            recorded.push(block);
        }
        sample_clock += frames;
    }
    Ok(Recording {
        start: CaptureStart::Reset,
        schema_version: 1,
        plugin_id: descriptor.id.into(),
        config,
        initial_state,
        reproducible_start: true,
        lost_blocks: drain.lost_blocks(),
        blocks: recorded,
    })
}
