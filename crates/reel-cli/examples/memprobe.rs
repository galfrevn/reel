//! Memory bisection: render N frames through the scratch path, optionally
//! feeding the GIF or WebM encoder, printing footprint as it goes.
use reel_format::ReelFile;
use reel_render::{plan, settings_from_config, Renderer};
use reel_timeline::Timeline;

fn footprint_mb() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap_or(0.0) / 1024.0
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: memprobe FILE.reel [render|gif|webm]");
    let mode = std::env::args().nth(2).unwrap_or_else(|| "render".into());
    let text = std::fs::read_to_string(&path).unwrap();
    let file = ReelFile::parse(&text).unwrap();
    let base = std::path::Path::new(&path).parent().unwrap();
    let cast = reel_cast::Cast::load(&base.join(&file.config.source.as_ref().unwrap().cast)).unwrap();
    let snaps = reel_term::replay(&cast).unwrap();
    let program = file.resolve(cast.duration()).unwrap();
    let (tl, _) = Timeline::compile(&program.edits, cast.duration()).unwrap();
    let (settings, _) = settings_from_config(&file.config).unwrap();
    let fps = settings.fps;
    let (mut r, _) = Renderer::new(settings).unwrap();
    let plans = plan(&tl, &snaps, &program.visuals, fps);
    println!("mode={mode} plans={} after-replay={:.0}MB", plans.len(), footprint_mb());

    let mut gif_builder = reel_encode::GifPaletteBuilder::new(256);
    let mut webm = None;
    for (i, f) in plans.iter().enumerate() {
        let (w, h, rgba) = r.render_frame_rgba(&snaps[f.snapshot], f);
        match mode.as_str() {
            "gif" => gif_builder.feed(rgba),
            "webm" => {
                let enc = webm.get_or_insert_with(|| {
                    reel_encode::WebmEncoder::new(w, h, &Default::default()).unwrap()
                });
                enc.push(rgba, w, h, f.dur).unwrap();
            }
            _ => {}
        }
        if i % 25 == 0 || i + 1 == plans.len() {
            println!("frame {i:4} → {:.0}MB", footprint_mb());
        }
    }
}
