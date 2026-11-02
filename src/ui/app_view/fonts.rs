//! Registering the editor and UI fonts with egui.

use super::*;

pub(super) fn configure_fonts(context: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    insert_static_font(
        &mut fonts,
        "moralerspace_neon_regular",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/fonts/MoralerspaceNeon-Regular.ttf"
        )),
        &[
            FontFamilyInsertion::MonospacePrimary,
            FontFamilyInsertion::ProportionalFallback,
        ],
    );
    context.set_fonts(fonts);

    let mut style = (*context.global_style()).clone();
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(14.0, egui::FontFamily::Monospace),
    );
    context.set_global_style(style);
}

#[derive(Clone, Copy)]
pub(super) enum FontFamilyInsertion {
    MonospacePrimary,
    ProportionalFallback,
}

pub(super) fn insert_static_font(
    fonts: &mut egui::FontDefinitions,
    name: &str,
    bytes: &'static [u8],
    insertions: &[FontFamilyInsertion],
) {
    fonts.font_data.insert(
        name.to_owned(),
        Arc::new(egui::FontData::from_static(bytes)),
    );

    for insertion in insertions {
        match insertion {
            FontFamilyInsertion::MonospacePrimary => {
                insert_font_family_name(fonts, egui::FontFamily::Monospace, name, 0);
            }
            FontFamilyInsertion::ProportionalFallback => {
                push_font_family_name(fonts, egui::FontFamily::Proportional, name);
            }
        }
    }
}

pub(super) fn insert_font_family_name(
    fonts: &mut egui::FontDefinitions,
    family: egui::FontFamily,
    name: &str,
    index: usize,
) {
    let family = fonts.families.entry(family).or_default();
    family.retain(|font_name| font_name != name);
    family.insert(index.min(family.len()), name.to_owned());
}

pub(super) fn push_font_family_name(
    fonts: &mut egui::FontDefinitions,
    family: egui::FontFamily,
    name: &str,
) {
    let family = fonts.families.entry(family).or_default();
    family.retain(|font_name| font_name != name);
    family.push(name.to_owned());
}
