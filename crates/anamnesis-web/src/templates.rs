//! The MiniJinja environment: every template embedded at compile time via
//! `include_str!`, so the built binary carries its templates with it and
//! `cargo run` never depends on a working directory containing `templates/`.

use minijinja::Environment;

/// Builds the environment with all of this crate's templates registered.
/// Panics if a template fails to parse — a syntax error in a template is a
/// build-time bug, not a runtime condition to recover from, so failing fast
/// at startup (this is called once, eagerly, before the server binds) is the
/// right behaviour.
pub fn build_environment() -> Environment<'static> {
    let mut env = Environment::new();
    for (name, source) in TEMPLATES {
        env.add_template(name, source)
            .unwrap_or_else(|e| panic!("template {name} failed to parse: {e}"));
    }
    env
}

const TEMPLATES: &[(&str, &str)] = &[
    ("base.html", include_str!("../templates/base.html")),
    ("boards.html", include_str!("../templates/boards.html")),
    ("board.html", include_str!("../templates/board.html")),
    ("_column.html", include_str!("../templates/_column.html")),
    ("_card.html", include_str!("../templates/_card.html")),
    ("login.html", include_str!("../templates/login.html")),
    ("error.html", include_str!("../templates/error.html")),
];

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::context;

    #[test]
    fn every_template_loads_without_panicking() {
        let _ = build_environment();
    }

    #[test]
    fn error_template_renders_status_and_message() {
        let env = build_environment();
        let tmpl = env.get_template("error.html").unwrap();
        let body = tmpl
            .render(context! { status => 404, message => "That board does not exist." })
            .unwrap();
        assert!(body.contains("404"));
        assert!(body.contains("That board does not exist."));
    }

    #[test]
    fn boards_template_html_escapes_titles() {
        // Auto-escape must be active for `.html` templates — a board titled
        // with a `<script>` tag must never render as live markup.
        let env = build_environment();
        let tmpl = env.get_template("boards.html").unwrap();
        let boards = vec![minijinja::context! { id => "abc", title => "<script>evil</script>" }];
        let body = tmpl
            .render(context! { boards => boards, csrf_token => "tok", current_user => "alice" })
            .unwrap();
        assert!(!body.contains("<script>evil</script>"));
        assert!(body.contains("&lt;script&gt;"));
    }

    #[test]
    fn login_template_renders_the_authorize_url() {
        let env = build_environment();
        let tmpl = env.get_template("login.html").unwrap();
        let body = tmpl
            .render(context! { authorize_url => "https://idp.example.com/authorize?x=1" })
            .unwrap();
        // MiniJinja's HTML escaper also escapes `/` (defense in depth against
        // attribute-breaking), so the URL survives but not byte-for-byte —
        // check for the host and query rather than the literal string.
        assert!(body.contains("idp.example.com"));
        assert!(body.contains("authorize?x=1"));
        assert!(body.contains(r#"href="https:&#x2f;&#x2f;idp.example.com&#x2f;authorize?x=1""#));
    }
}
