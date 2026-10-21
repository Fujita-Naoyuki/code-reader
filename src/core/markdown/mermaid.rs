//! Mermaid code blocks rendered to SVG (through `mmdflux`) for the preview.

pub(crate) fn is_mermaid_language(language: &str) -> bool {
    language.eq_ignore_ascii_case("mermaid") || language.eq_ignore_ascii_case("mmd")
}

pub(crate) fn render_mermaid_svg(code: &str) -> Result<String, String> {
    mmdflux::render_diagram(
        code,
        mmdflux::OutputFormat::Svg,
        &mmdflux::RenderConfig::default(),
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_mermaid_renderer_outputs_svg_for_flowchart() {
        let svg = render_mermaid_svg("graph TD\n    A[Start] --> B[End]").unwrap();

        assert!(svg.contains("<svg"));
        assert!(svg.contains("Start"));
    }
}
