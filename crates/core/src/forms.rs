//! Filling out PDF forms (AcroForm): text fields, checkboxes, radio buttons, dropdowns and lists.
//!
//! Values are set through MuPDF's form API, which stores them the standard way (`/V`, and
//! `/AS` for buttons) and regenerates each field's appearance, so every PDF reader shows what
//! was filled in. Scripts in the form (calculated totals, formatting) are run by MuPDF's
//! JavaScript engine where the form has them.
//!
//! Not supported: XFA forms (an old Adobe-only format) and typing a custom value into an
//! editable dropdown.

use mupdf::pdf::{FieldFlags, PdfObject, PdfPage, PdfWidget, WidgetType};

use crate::engine::{Engine, Result, err};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text { multiline: bool, password: bool },
    CheckBox,
    Radio,
    /// A dropdown: one choice from a list.
    Dropdown,
    /// A list box: one choice (or several, see `multi_select`).
    List { multi_select: bool },
    /// Push buttons and signature fields: shown, but not something to fill in.
    Other,
}

#[derive(Clone, Debug)]
pub struct FormField {
    pub page: usize,
    /// PDF object number of the field's widget on this page.
    pub xref: i32,
    pub kind: FieldKind,
    pub name: String,
    /// The form author's description of the field (tooltip), if any.
    pub label: String,
    /// Text of a text field, or the chosen option of a dropdown / list.
    pub value: String,
    /// Checkboxes and radio buttons: is this one on?
    pub checked: bool,
    /// Dropdowns and lists: (stored value, text shown).
    pub options: Vec<(String, String)>,
    /// Box on the page in points: (x0, y0, x1, y1), top-left origin.
    pub rect: (f32, f32, f32, f32),
    pub read_only: bool,
    pub required: bool,
}

/// The name of a button widget's "on" appearance (for example `Yes`, `1` or an export value).
fn on_state(widget: &PdfObject) -> Option<String> {
    let normal = widget.get_dict("AP").ok()??.get_dict("N").ok()??;
    for i in 0..normal.dict_len().ok()? {
        let key = normal.get_dict_key(i as i32).ok()??.as_name().ok()?;
        if key != b"Off" {
            return Some(String::from_utf8_lossy(&key).into_owned());
        }
    }
    None
}

fn options(widget: &PdfObject) -> Vec<(String, String)> {
    let Ok(Some(list)) = widget.get_dict_inheritable("Opt") else { return Vec::new() };
    let text = |o: &PdfObject| o.as_string().unwrap_or_default();
    (0..list.len().unwrap_or(0))
        .filter_map(|i| list.get_array(i as i32).ok().flatten())
        .map(|item| {
            if item.is_array().unwrap_or(false) {
                // [stored value, text shown]
                let part = |i| item.get_array(i).ok().flatten().map(|o| text(&o)).unwrap_or_default();
                (part(0), part(1))
            } else {
                (text(&item), text(&item))
            }
        })
        .collect()
}

fn describe(page: usize, widget: &PdfWidget) -> Option<FormField> {
    let flags = widget.field_flags().unwrap_or(FieldFlags::empty());
    let object = widget.annotation().object();
    let kind = match widget.r#type().ok()? {
        WidgetType::Text => FieldKind::Text {
            multiline: flags.contains(FieldFlags::MULTILINE),
            password: flags.contains(FieldFlags::PASSWORD),
        },
        WidgetType::Checkbox => FieldKind::CheckBox,
        WidgetType::RadioButton => FieldKind::Radio,
        WidgetType::Combobox => FieldKind::Dropdown,
        WidgetType::Listbox => FieldKind::List { multi_select: flags.contains(FieldFlags::MULTI_SELECT) },
        _ => FieldKind::Other,
    };
    let r = widget.annotation().rect().or_else(|_| widget.annotation().bounds()).ok()?;
    let checked = matches!(kind, FieldKind::CheckBox | FieldKind::Radio)
        && object
            .get_dict("AS")
            .ok()
            .flatten()
            .and_then(|s| s.as_name().ok())
            .is_some_and(|state| state != b"Off");
    Some(FormField {
        page,
        xref: widget.xref().ok()?,
        kind,
        name: widget.name().ok().flatten().unwrap_or_default(),
        label: widget.label().ok().flatten().unwrap_or_default(),
        value: widget.value().ok().flatten().unwrap_or_default(),
        checked,
        options: match kind {
            FieldKind::Dropdown | FieldKind::List { .. } => options(&object),
            _ => Vec::new(),
        },
        rect: (r.x0, r.y0, r.x1, r.y1),
        read_only: widget.is_readonly().unwrap_or(false) || flags.contains(FieldFlags::READ_ONLY),
        required: flags.contains(FieldFlags::REQUIRED),
    })
}

impl Engine {
    /// The form fields on one page. Empty for documents without a form.
    pub fn page_form_fields(&self, page: usize) -> Vec<FormField> {
        let Some(doc) = self.pdf() else { return Vec::new() };
        let Ok(p) = doc.load_pdf_page(page as i32) else { return Vec::new() };
        p.widgets().filter_map(|w| describe(page, &w)).collect()
    }

    /// Every form field in the document, page by page. Cheap when there is no form.
    pub fn form_fields(&self) -> Vec<FormField> {
        let Some(doc) = self.pdf() else { return Vec::new() };
        if !doc.has_acro_form().unwrap_or(false) {
            return Vec::new();
        }
        (0..self.page_count()).flat_map(|page| self.page_form_fields(page)).collect()
    }

    fn field_widget(&mut self, page: usize, xref: i32) -> Result<(PdfPage, PdfWidget)> {
        if page >= self.page_count() {
            return Err(format!("Page {} does not exist", page + 1));
        }
        let p = self.pdf_mut()?.load_pdf_page(page as i32).map_err(err)?;
        let widget = p
            .widgets()
            .find(|w| w.xref().is_ok_and(|x| x == xref))
            .ok_or("That form field no longer exists")?;
        if widget.is_readonly().unwrap_or(false) {
            return Err("This field cannot be changed".into());
        }
        Ok((p, widget))
    }

    fn after_field_change(&mut self, mut page: PdfPage, mut widget: PdfWidget) {
        let _ = widget.update();
        drop(widget);
        let _ = page.update();
        // Recalculate fields that depend on others (totals and the like), if the form has any.
        if let Ok(doc) = self.pdf_mut() {
            let _ = doc.calculate_form();
            // Some forms also carry an XFA copy (an old Adobe-only format). Readers that
            // prefer it would show the form as still empty, so it is removed once the form
            // is being filled in here; the standard form stays.
            if let Ok(Some(mut form)) = doc.catalog().and_then(|c| c.get_dict("AcroForm")) {
                if matches!(form.get_dict("XFA"), Ok(Some(_))) {
                    let _ = form.dict_delete("XFA");
                }
            }
        }
        self.mark_dirty();
    }

    /// Type into a text field, or choose an option of a dropdown or list (by its stored value).
    pub fn set_field_value(&mut self, page: usize, xref: i32, value: &str) -> Result<()> {
        let (p, mut widget) = self.field_widget(page, xref)?;
        match widget.r#type().map_err(err)? {
            WidgetType::Text => {}
            WidgetType::Combobox | WidgetType::Listbox => {
                let known = options(&widget.annotation().object());
                if !value.is_empty() && !known.iter().any(|(stored, _)| stored == value) {
                    return Err("That is not one of the choices of this field".into());
                }
            }
            _ => return Err("This field does not take text".into()),
        }
        let accepted = widget.set_value(self.pdf_mut()?, value, false).map_err(err)?;
        if !accepted {
            return Err("The form did not accept this value".into());
        }
        self.after_field_change(p, widget);
        Ok(())
    }

    /// Tick or clear a checkbox, or select a radio button (radio buttons cannot be cleared,
    /// only another one of the group chosen).
    pub fn set_field_checked(&mut self, page: usize, xref: i32, checked: bool) -> Result<()> {
        let (p, widget) = self.field_widget(page, xref)?;
        let kind = widget.r#type().map_err(err)?;
        let state = match (kind, checked) {
            (WidgetType::Checkbox | WidgetType::RadioButton, true) => {
                on_state(&widget.annotation().object()).ok_or("This box has no checked state")?
            }
            (WidgetType::Checkbox, false) => "Off".to_owned(),
            (WidgetType::RadioButton, false) => return Err("Choose another option instead".into()),
            _ => return Err("This field is not a checkbox".into()),
        };
        // Buttons are switched by hand so that a whole group stays consistent: the field's
        // value becomes the chosen state, and every button of the group shows either that
        // state (if it is its own "on" state) or "Off".
        let doc = self.pdf_mut()?;
        let this = widget.annotation().object();
        let parent = this.get_dict("Parent").map_err(err)?;
        let is_kid = parent.is_some() && this.get_dict("T").map_err(err)?.is_none();
        let (mut field, buttons) = match (is_kid, parent) {
            (true, Some(group)) => {
                let kids = group.get_dict("Kids").map_err(err)?;
                let buttons = kids
                    .map(|k| (0..k.len().unwrap_or(0)).filter_map(|i| k.get_array(i as i32).ok().flatten()).collect())
                    .unwrap_or_default();
                (group, buttons)
            }
            _ => (widget.annotation().object(), vec![widget.annotation().object()]),
        };
        field.dict_put("V", doc.new_name(&state).map_err(err)?).map_err(err)?;
        for mut button in buttons {
            let shows = if on_state(&button).as_deref() == Some(state.as_str()) { state.as_str() } else { "Off" };
            button.dict_put("AS", doc.new_name(shows).map_err(err)?).map_err(err)?;
        }
        self.after_field_change(p, widget);
        Ok(())
    }
}
