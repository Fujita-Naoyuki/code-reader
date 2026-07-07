use eframe::egui;

use crate::{
    languages::{GoLanguage, LanguageRegistry, RustLanguage},
    ui::app_view::CodeReaderApp,
};

#[derive(Debug)]
pub struct AppError {
    message: String,
}

impl std::fmt::Display for AppError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

pub fn run() -> Result<(), AppError> {
    let registry =
        LanguageRegistry::with_languages(vec![Box::new(RustLanguage), Box::new(GoLanguage)]);
    #[cfg(debug_assertions)]
    {
        println!("code-reader starting");
        println!("registered languages: {}", registry.len());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Code Reader")
            .with_inner_size([1500.0, 820.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Code Reader",
        options,
        Box::new(move |creation_context| {
            Ok(Box::new(CodeReaderApp::new(
                creation_context,
                registry.clone(),
            )))
        }),
    )
    .map_err(|error| AppError {
        message: error.to_string(),
    })?;

    Ok(())
}
