use scraper::{Html, Selector};
use std::collections::HashMap;

use super::error::ErrorCode;
use super::page::HttpMethod;
use crate::service::service::ServiceError;

/// A parsed form from the DOM.
#[derive(Debug, Clone)]
pub struct ParsedForm {
    pub action: Option<String>,
    pub method: HttpMethod,
    #[allow(dead_code)]
    pub enctype: FormEnctype,
    /// Default field values from the DOM: (name → value).
    pub fields: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FormEnctype {
    UrlEncoded,
    Multipart,
}

/// Find a `<form>` by CSS selector and extract its attributes and default field values.
pub fn parse_form(html: &str, form_selector: &str) -> Result<ParsedForm, FormError> {
    let document = Html::parse_document(html);

    let sel = Selector::parse(form_selector)
        .map_err(|_| FormError::InvalidSelector(form_selector.to_string()))?;

    let form_el = document
        .select(&sel)
        .next()
        .ok_or_else(|| FormError::FormNotFound(form_selector.to_string()))?;

    if form_el.value().name() != "form" {
        return Err(FormError::NotAForm(form_selector.to_string()));
    }

    let action = form_el.value().attr("action").map(|s| s.to_string());

    let method = match form_el
        .value()
        .attr("method")
        .map(|m| m.to_ascii_uppercase())
        .as_deref()
    {
        Some("POST") => HttpMethod::Post,
        Some("PUT") => HttpMethod::Put,
        Some("PATCH") => HttpMethod::Patch,
        Some("DELETE") => HttpMethod::Delete,
        _ => HttpMethod::Get,
    };

    let enctype = match form_el.value().attr("enctype") {
        Some("multipart/form-data") => FormEnctype::Multipart,
        _ => FormEnctype::UrlEncoded,
    };

    let mut fields = HashMap::new();

    let input_sel = Selector::parse("input").unwrap();
    for input in form_el.select(&input_sel) {
        let name = match input.value().attr("name") {
            Some(n) if !n.is_empty() => n,
            _ => continue,
        };
        let input_type = input
            .value()
            .attr("type")
            .unwrap_or("text")
            .to_ascii_lowercase();

        match input_type.as_str() {
            "submit" | "button" | "image" | "reset" | "file" => continue,
            "checkbox" | "radio" => {
                if input.value().attr("checked").is_some() {
                    let val = input.value().attr("value").unwrap_or("on");
                    fields.insert(name.to_string(), val.to_string());
                }
            }
            _ => {
                let val = input.value().attr("value").unwrap_or("");
                fields.insert(name.to_string(), val.to_string());
            }
        }
    }

    let textarea_sel = Selector::parse("textarea").unwrap();
    for textarea in form_el.select(&textarea_sel) {
        let name = match textarea.value().attr("name") {
            Some(n) if !n.is_empty() => n,
            _ => continue,
        };
        let text: String = textarea.text().collect();
        fields.insert(name.to_string(), text);
    }

    let select_sel = Selector::parse("select").unwrap();
    let option_sel = Selector::parse("option").unwrap();
    for select in form_el.select(&select_sel) {
        let name = match select.value().attr("name") {
            Some(n) if !n.is_empty() => n,
            _ => continue,
        };
        let selected = select
            .select(&option_sel)
            .find(|o| o.value().attr("selected").is_some())
            .or_else(|| select.select(&option_sel).next());

        if let Some(opt) = selected {
            let val = opt
                .value()
                .attr("value")
                .map(|s| s.to_string())
                .unwrap_or_else(|| opt.text().collect());
            fields.insert(name.to_string(), val);
        }
    }

    Ok(ParsedForm {
        action,
        method,
        enctype,
        fields,
    })
}

/// Validate that a CSS selector matches an input, textarea, or select element
/// in a pre-parsed HTML document. Returns the field's `name` attribute.
pub fn validate_field_selector(document: &Html, selector: &str) -> Result<String, FormError> {
    let sel = Selector::parse(selector)
        .map_err(|_| FormError::InvalidSelector(selector.to_string()))?;

    let element = document
        .select(&sel)
        .next()
        .ok_or_else(|| FormError::SelectorNotFound(selector.to_string()))?;

    let tag = element.value().name();
    match tag {
        "input" | "textarea" | "select" => {}
        _ => {
            return Err(FormError::NotAField(format!(
                "selector '{selector}' matched <{tag}>, expected <input>, <textarea>, or <select>"
            )));
        }
    }

    let name = element
        .value()
        .attr("name")
        .ok_or_else(|| {
            FormError::NotAField(format!(
                "element matching '{selector}' has no name attribute"
            ))
        })?
        .to_string();

    Ok(name)
}

/// Build a URL-encoded form body from field name→value pairs.
pub fn encode_form_urlencoded(fields: &HashMap<String, String>) -> String {
    let mut pairs: Vec<_> = fields.iter().collect();
    pairs.sort_by_key(|(k, _)| k.as_str());
    form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
}

/// Convert a FormError to the appropriate ServiceError.
pub fn map_form_error(e: FormError) -> ServiceError {
    let code = match &e {
        FormError::SelectorNotFound(_) => ErrorCode::SelectorNotFound,
        FormError::InvalidSelector(_) => ErrorCode::InvalidCommand,
        FormError::FormNotFound(_) | FormError::NotAForm(_) | FormError::NotAField(_) => {
            ErrorCode::FormNotFound
        }
    };
    ServiceError::new(code, e.to_string())
}

#[derive(Debug, thiserror::Error)]
pub enum FormError {
    #[error("invalid selector: {0}")]
    InvalidSelector(String),
    #[error("form not found: {0}")]
    FormNotFound(String),
    #[error("selector '{0}' does not match a <form> element")]
    NotAForm(String),
    #[error("selector not found: {0}")]
    SelectorNotFound(String),
    #[error("{0}")]
    NotAField(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORM_HTML: &str = r#"
    <html>
    <body>
        <form id="login" action="/login" method="POST">
            <input type="text" name="username" value="">
            <input type="password" name="password" value="">
            <input type="hidden" name="csrf" value="tok123">
            <input type="submit" value="Log In">
        </form>
    </body>
    </html>
    "#;

    const FORM_WITH_SELECT: &str = r#"
    <html>
    <body>
        <form id="settings" action="/save" method="POST">
            <input type="text" name="name" value="Alice">
            <textarea name="bio">Hello world</textarea>
            <select name="role">
                <option value="user">User</option>
                <option value="admin" selected>Admin</option>
            </select>
            <input type="checkbox" name="newsletter" value="yes" checked>
            <input type="checkbox" name="terms" value="yes">
        </form>
    </body>
    </html>
    "#;

    const MULTIPART_FORM: &str = r#"
    <html>
    <body>
        <form id="upload" action="/upload" method="POST" enctype="multipart/form-data">
            <input type="text" name="title" value="">
            <input type="file" name="attachment">
            <input type="submit" value="Upload">
        </form>
    </body>
    </html>
    "#;

    #[test]
    fn test_parse_login_form() {
        let form = parse_form(FORM_HTML, "form#login").unwrap();
        assert_eq!(form.action, Some("/login".into()));
        assert_eq!(form.method, HttpMethod::Post);
        assert_eq!(form.enctype, FormEnctype::UrlEncoded);
        assert_eq!(form.fields.get("username").unwrap(), "");
        assert_eq!(form.fields.get("password").unwrap(), "");
        assert_eq!(form.fields.get("csrf").unwrap(), "tok123");
        assert!(!form.fields.contains_key("submit")); // submit button excluded
    }

    #[test]
    fn test_parse_form_with_select_textarea_checkbox() {
        let form = parse_form(FORM_WITH_SELECT, "form#settings").unwrap();
        assert_eq!(form.fields.get("name").unwrap(), "Alice");
        assert_eq!(form.fields.get("bio").unwrap(), "Hello world");
        assert_eq!(form.fields.get("role").unwrap(), "admin"); // selected option
        assert_eq!(form.fields.get("newsletter").unwrap(), "yes"); // checked
        assert!(!form.fields.contains_key("terms")); // not checked
    }

    #[test]
    fn test_parse_multipart_form() {
        let form = parse_form(MULTIPART_FORM, "form#upload").unwrap();
        assert_eq!(form.enctype, FormEnctype::Multipart);
        assert_eq!(form.fields.get("title").unwrap(), "");
        assert!(!form.fields.contains_key("attachment")); // file inputs excluded
    }

    #[test]
    fn test_parse_form_not_found() {
        let err = parse_form(FORM_HTML, "form#nonexistent").unwrap_err();
        assert!(matches!(err, FormError::FormNotFound(_)));
    }

    #[test]
    fn test_parse_form_not_a_form() {
        let html = r#"<html><body><div id="login">not a form</div></body></html>"#;
        let err = parse_form(html, "#login").unwrap_err();
        assert!(matches!(err, FormError::NotAForm(_)));
    }

    #[test]
    fn test_parse_form_invalid_selector() {
        let err = parse_form(FORM_HTML, "[[[invalid").unwrap_err();
        assert!(matches!(err, FormError::InvalidSelector(_)));
    }

    #[test]
    fn test_parse_form_get_method_default() {
        let html = r#"<html><body><form id="f" action="/search"><input name="q" value=""></form></body></html>"#;
        let form = parse_form(html, "form#f").unwrap();
        assert_eq!(form.method, HttpMethod::Get);
    }

    #[test]
    fn test_validate_field_selector() {
        let doc = Html::parse_document(FORM_HTML);
        let name = validate_field_selector(&doc, "input[name='username']").unwrap();
        assert_eq!(name, "username");
    }

    #[test]
    fn test_validate_field_selector_textarea() {
        let doc = Html::parse_document(FORM_WITH_SELECT);
        let name = validate_field_selector(&doc, "textarea[name='bio']").unwrap();
        assert_eq!(name, "bio");
    }

    #[test]
    fn test_validate_field_selector_select() {
        let doc = Html::parse_document(FORM_WITH_SELECT);
        let name = validate_field_selector(&doc, "select[name='role']").unwrap();
        assert_eq!(name, "role");
    }

    #[test]
    fn test_validate_field_selector_not_found() {
        let doc = Html::parse_document(FORM_HTML);
        let err = validate_field_selector(&doc, "input[name='nope']").unwrap_err();
        assert!(matches!(err, FormError::SelectorNotFound(_)));
    }

    #[test]
    fn test_validate_field_selector_not_a_field() {
        let doc = Html::parse_document(FORM_HTML);
        let err = validate_field_selector(&doc, "form#login").unwrap_err();
        assert!(matches!(err, FormError::NotAField(_)));
    }

    #[test]
    fn test_encode_form_urlencoded() {
        let mut fields = HashMap::new();
        fields.insert("username".into(), "agent@example.com".into());
        fields.insert("password".into(), "s3cret!".into());
        let encoded = encode_form_urlencoded(&fields);
        assert!(encoded.contains("username=agent%40example.com"));
        assert!(encoded.contains("password=s3cret%21"));
    }

    #[test]
    fn test_encode_form_urlencoded_sorted() {
        let mut fields = HashMap::new();
        fields.insert("z_field".into(), "last".into());
        fields.insert("a_field".into(), "first".into());
        let encoded = encode_form_urlencoded(&fields);
        assert!(encoded.starts_with("a_field="));
    }

    #[test]
    fn test_parse_form_radio_checked() {
        let html = r#"
        <html><body>
        <form id="f" action="/pick" method="POST">
            <input type="radio" name="color" value="red">
            <input type="radio" name="color" value="blue" checked>
            <input type="radio" name="color" value="green">
        </form>
        </body></html>"#;
        let form = parse_form(html, "form#f").unwrap();
        assert_eq!(form.fields.get("color").unwrap(), "blue");
    }

    #[test]
    fn test_parse_form_select_no_selected() {
        let html = r#"
        <html><body>
        <form id="f" action="/pick" method="POST">
            <select name="size">
                <option value="s">Small</option>
                <option value="m">Medium</option>
            </select>
        </form>
        </body></html>"#;
        let form = parse_form(html, "form#f").unwrap();
        assert_eq!(form.fields.get("size").unwrap(), "s");
    }
}
