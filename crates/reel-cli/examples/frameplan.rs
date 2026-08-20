//! Debug helper: print the frame plan (out_t, dur) for a .reel file.
use reel_format::ReelFile;
use reel_render::{plan, settings_from_config};
use reel_timeline::Timeline;

fn main() {
    let path = std::env::args().nth(1).expect("usage: frameplan FILE.reel");
    let text = std::fs::read_to_string(&path).unwrap();
    let file = ReelFile::parse(&text).unwrap();
    let base = std::path::Path::new(&path).parent().unwrap();
    let cast = reel_cast::Cast::load(&base.join(&file.config.source.as_ref().unwrap().cast)).unwrap();
    let snaps = reel_term::replay(&cast).unwrap();
    let program = file.resolve(cast.duration()).unwrap();
    let (tl, _) = Timeline::compile(&program.edits, cast.duration()).unwrap();
    let (settings, _) = settings_from_config(&file.config).unwrap();
    let frames = plan(&tl, &snaps, &program.visuals, settings.fps);
    let mut cum = 0.0f64;
    println!("idx  out_t     cum_dur   drift_ms  dur");
    for (i, f) in frames.iter().enumerate() {
        let drift = (cum - f.out_t) * 1000.0;
        if drift.abs() > 1.0 || (4.0..6.2).contains(&f.out_t) {
            println!("{i:3}  {:8.3}  {cum:8.3}  {drift:8.1}  {:.3}", f.out_t, f.dur);
        }
        cum += f.dur;
    }
    println!("total frames {} cum {:.3} out_dur {:.3}", frames.len(), cum, tl.out_duration());
}
