mod app;
mod render3d;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1580.0, 980.0])
            .with_min_inner_size([1180.0, 720.0])
            .with_title("Spatial DAW"),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "Spatial DAW",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
