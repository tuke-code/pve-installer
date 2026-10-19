use std::{collections::HashMap, net::IpAddr, str::FromStr, sync::Arc};

use cursive::{
    Printer, Rect, Vec2, View,
    event::{Event, EventResult},
    theme::{BaseColor, StyleType},
    view::{Resizable, ViewWrapper},
    views::{
        Checkbox, DummyView, EditView, LinearLayout, NamedView, ResizedView, SelectView, TextView,
    },
};

mod bootdisk;
pub use bootdisk::*;

mod install_progress;
pub use install_progress::*;

mod network;
pub use network::*;

mod tabbed_view;
use proxmox_network_types::ip_address::Cidr;
pub use tabbed_view::*;

mod table_view;
pub use table_view::*;

mod timezone;
pub use timezone::*;

pub struct NumericEditView<T> {
    view: LinearLayout,
    min_value: Option<T>,
    max_value: Option<T>,
    placeholder: Option<T>,
    max_content_width: Option<usize>,
    allow_empty: bool,
}

impl<T: Copy + ToString + FromStr + PartialOrd> NumericEditView<T> {
    /// Creates a new [`NumericEditView`], with the value set to `0`.
    pub fn new() -> Self {
        let view = LinearLayout::horizontal().child(EditView::new().content("0").full_width());

        Self {
            view,
            min_value: None,
            max_value: None,
            placeholder: None,
            max_content_width: None,
            allow_empty: false,
        }
    }

    /// Creates a new [`NumericEditView`], with the value set to `0` and a label to the right of it
    /// with the given content, separated by a space.
    ///
    /// # Arguments
    /// * `suffix` - Content for the label to the right of it.
    pub fn new_with_suffix(suffix: &str) -> Self {
        let view = LinearLayout::horizontal()
            .child(EditView::new().content("0").full_width())
            .child(TextView::new(format!(" {suffix}")));

        Self {
            view,
            min_value: None,
            max_value: None,
            placeholder: None,
            max_content_width: None,
            allow_empty: false,
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.inner_mut().set_enabled(enabled);
        self
    }

    pub fn min_value(mut self, min: T) -> Self {
        self.min_value = Some(min);
        self
    }

    pub fn max_value(mut self, max: T) -> Self {
        self.max_value = Some(max);
        self
    }

    pub fn max_content_width(mut self, width: usize) -> Self {
        self.max_content_width = Some(width);
        self.inner_mut().set_max_content_width(Some(width));
        self
    }

    pub fn allow_empty(mut self, value: bool) -> Self {
        self.allow_empty = value;

        if value {
            *self.inner_mut() = EditView::new();
        } else {
            *self.inner_mut() = EditView::new().content("0");
        }

        let max_content_width = self.max_content_width;
        self.inner_mut().set_max_content_width(max_content_width);
        self
    }

    /// Sets a placeholder value for this view. Implies `allow_empty(true)`.
    /// Implies `allow_empty(true)`.
    ///
    /// # Arguments
    /// `placeholder` - The placeholder value to set for this view.
    pub fn placeholder(mut self, placeholder: T) -> Self {
        self.placeholder = Some(placeholder);
        self.allow_empty(true)
    }

    /// Returns the current value of the view. If a placeholder is defined and
    /// no value is currently set, the placeholder value is returned.
    ///
    /// **This should only be called when `allow_empty = false` or a placeholder
    /// is set.**
    pub fn get_content(&self) -> Result<T, <T as FromStr>::Err> {
        let content = self.inner().get_content();

        if content.is_empty()
            && let Some(placeholder) = self.placeholder
        {
            return Ok(placeholder);
        }

        assert!(!(self.allow_empty && self.placeholder.is_none()));
        content.parse()
    }

    /// Returns the current value of the view, or [`None`] if the view is
    /// currently empty.
    pub fn get_content_maybe(&self) -> Option<Result<T, <T as FromStr>::Err>> {
        let content = self.inner().get_content();

        if !content.is_empty() {
            Some(content.parse())
        } else {
            None
        }
    }

    pub fn set_max_value(&mut self, max: T) {
        self.max_value = Some(max);
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.inner_mut().set_enabled(enabled);
    }

    /// Checks if the new value is actually valid according to the min and max value, if set
    fn check_bounds(&mut self, original: Arc<String>, result: EventResult) -> EventResult {
        let value = self.get_content_maybe().and_then(Result::ok);
        let above_min = self
            .min_value
            .zip(value)
            .is_none_or(|(min, val)| val >= min);
        let below_max = self
            .max_value
            .zip(value)
            .is_none_or(|(max, val)| val <= max);

        if result.is_consumed() && (!above_min || !below_max) {
            // Restore the original value, before the insert
            let cb = self.inner_mut().set_content((*original).clone());
            return EventResult::with_cb_once(move |siv| {
                result.process(siv);
                cb(siv);
            });
        }

        result
    }

    /// Provides an immutable reference to the inner [`EditView`].
    fn inner(&self) -> &EditView {
        // Safety: Invariant; first child must always exist and be a `EditView`
        self.view
            .get_child(0)
            .unwrap()
            .downcast_ref::<ResizedView<EditView>>()
            .unwrap()
            .get_inner()
    }

    /// Provides a mutable reference to the inner [`EditView`].
    fn inner_mut(&mut self) -> &mut EditView {
        // Safety: Invariant; first child must always exist and be a `EditView`
        self.view
            .get_child_mut(0)
            .unwrap()
            .downcast_mut::<ResizedView<EditView>>()
            .unwrap()
            .get_inner_mut()
    }

    /// Sets the content of the inner [`EditView`]. This correctly swaps out the content without
    /// modifying the [`EditView`] in any way.
    ///
    /// Chainable variant.
    ///
    /// # Arguments
    /// * `content` - New, stringified content for the inner [`EditView`]. Must be a valid value
    ///   according to the container type `T`.
    fn content_inner(mut self, content: &str) -> Self {
        let mut inner = EditView::new();
        std::mem::swap(self.inner_mut(), &mut inner);
        inner = inner.content(content);
        std::mem::swap(self.inner_mut(), &mut inner);
        self
    }

    /// Generic `wrap_draw()` implementation for [`ViewWrapper`].
    ///
    /// # Arguments
    /// * `printer` - The [`Printer`] to draw to the base view.
    fn wrap_draw_inner(&self, printer: &Printer) {
        self.view.draw(printer);

        if self.inner().get_content().is_empty()
            && !printer.focused
            && let Some(placeholder) = self.placeholder
        {
            let placeholder = placeholder.to_string();

            printer.with_color(
                (BaseColor::Blue.light(), BaseColor::Blue.dark()).into(),
                |printer| printer.print((0, 0), &placeholder),
            );
        }
    }
}

pub type FloatEditView = NumericEditView<f64>;
pub type IntegerEditView = NumericEditView<usize>;

impl ViewWrapper for FloatEditView {
    cursive::wrap_impl!(self.view: LinearLayout);

    fn wrap_draw(&self, printer: &Printer) {
        self.wrap_draw_inner(printer);
    }

    fn wrap_on_event(&mut self, event: Event) -> EventResult {
        let original = self.inner_mut().get_content();

        let has_decimal_place = original.find('.').is_some();

        let result = match event {
            Event::Char(c) if !c.is_numeric() && c != '.' => return EventResult::consumed(),
            Event::Char('.') if has_decimal_place => return EventResult::consumed(),
            _ => self.view.on_event(event),
        };

        let decimal_places = self
            .inner_mut()
            .get_content()
            .split_once('.')
            .map(|(_, s)| s.len())
            .unwrap_or_default();
        if decimal_places > 2 {
            let cb = self.inner_mut().set_content((*original).clone());
            return EventResult::with_cb_once(move |siv| {
                result.process(siv);
                cb(siv);
            });
        }

        self.check_bounds(original, result)
    }
}

impl FloatEditView {
    /// Sets the value of the [`FloatEditView`].
    pub fn content(self, content: f64) -> Self {
        self.content_inner(&format!("{:.2}", content))
    }
}

impl ViewWrapper for IntegerEditView {
    cursive::wrap_impl!(self.view: LinearLayout);

    fn wrap_draw(&self, printer: &Printer) {
        self.wrap_draw_inner(printer);
    }

    fn wrap_on_event(&mut self, event: Event) -> EventResult {
        let original = self.inner_mut().get_content();

        let result = match event {
            // Drop all other characters than numbers; allow dots if not set to integer-only
            Event::Char(c) if !c.is_numeric() => EventResult::consumed(),
            _ => self.view.on_event(event),
        };

        self.check_bounds(original, result)
    }
}

impl IntegerEditView {
    /// Sets the value of the [`IntegerEditView`].
    pub fn content(self, content: usize) -> Self {
        self.content_inner(&content.to_string())
    }
}

pub struct DiskSizeEditView {
    view: LinearLayout,
    allow_empty: bool,
}

impl DiskSizeEditView {
    pub fn new() -> Self {
        let view = LinearLayout::horizontal()
            .child(FloatEditView::new().full_width())
            .child(TextView::new(" GB"));

        Self {
            view,
            allow_empty: false,
        }
    }

    pub fn new_emptyable() -> Self {
        let view = LinearLayout::horizontal()
            .child(FloatEditView::new().allow_empty(true).full_width())
            .child(TextView::new(" GB"));

        Self {
            view,
            allow_empty: true,
        }
    }

    pub fn content(mut self, content: f64) -> Self {
        if let Some(view) = self.view.get_child_mut(0).and_then(|v| v.downcast_mut()) {
            *view = FloatEditView::new().content(content).full_width();
        }

        self
    }

    pub fn content_maybe(self, content: Option<f64>) -> Self {
        if let Some(value) = content {
            self.content(value)
        } else {
            self
        }
    }

    pub fn max_value(mut self, max: f64) -> Self {
        if let Some(view) = self
            .view
            .get_child_mut(0)
            .and_then(|v| v.downcast_mut::<ResizedView<FloatEditView>>())
        {
            view.get_inner_mut().set_max_value(max);
        }

        self
    }

    pub fn get_content(&self) -> Option<f64> {
        self.with_view(|v| {
            v.get_child(0)?
                .downcast_ref::<ResizedView<FloatEditView>>()?
                .with_view(|v| {
                    if self.allow_empty {
                        v.get_content_maybe().and_then(Result::ok)
                    } else {
                        v.get_content().ok()
                    }
                })
                .flatten()
        })
        .flatten()
    }
}

impl ViewWrapper for DiskSizeEditView {
    cursive::wrap_impl!(self.view: LinearLayout);
}

pub trait FormViewGetValue<R> {
    fn get_value(&self) -> Option<R>;
}

impl FormViewGetValue<String> for EditView {
    fn get_value(&self) -> Option<String> {
        Some((*self.get_content()).clone())
    }
}

impl<T: 'static + Clone + Send + Sync> FormViewGetValue<T> for SelectView<T> {
    fn get_value(&self) -> Option<T> {
        self.selection().map(|v| (*v).clone())
    }
}

impl<T> FormViewGetValue<T> for NumericEditView<T>
where
    T: Copy + ToString + FromStr + PartialOrd,
    NumericEditView<T>: ViewWrapper,
{
    fn get_value(&self) -> Option<T> {
        self.get_content().ok()
    }
}

impl FormViewGetValue<(IpAddr, u8)> for CidrAddressEditView {
    // FIXME: return Cidr (again) with proper error handling through Result
    fn get_value(&self) -> Option<(IpAddr, u8)> {
        self.get_values()
    }
}

impl<T, R> FormViewGetValue<R> for NamedView<T>
where
    T: 'static + FormViewGetValue<R>,
    NamedView<T>: ViewWrapper,
    <NamedView<T> as ViewWrapper>::V: FormViewGetValue<R>,
{
    fn get_value(&self) -> Option<R> {
        self.with_view(|v| v.get_value()).flatten()
    }
}

impl FormViewGetValue<f64> for DiskSizeEditView {
    fn get_value(&self) -> Option<f64> {
        self.get_content()
    }
}

pub struct FormView<UDT = ()> {
    view: LinearLayout,
    user_data: HashMap<usize, UDT>,
}

impl<UDT> FormView<UDT> {
    pub fn new() -> Self {
        let view = LinearLayout::horizontal()
            .child(LinearLayout::vertical().full_width())
            .child(LinearLayout::vertical().full_width());

        Self {
            view,
            user_data: HashMap::new(),
        }
    }

    pub fn add_child(&mut self, label: &str, view: impl View) {
        self.add_to_column(0, TextView::new(format!("{label}: ")).no_wrap());
        self.add_to_column(1, view);
    }

    pub fn add_child_with_custom_label(&mut self, label: &str, view: impl View) {
        self.add_to_column(0, TextView::new(label).no_wrap());
        self.add_to_column(1, view);
    }

    pub fn add_child_with_data(&mut self, label: &str, view: impl View, data: UDT) {
        self.add_to_column(0, TextView::new(format!("{label}: ")).no_wrap());
        self.add_to_column(1, view);
        self.user_data.insert(self.len() - 1, data);
    }

    pub fn child(mut self, label: &str, view: impl View) -> Self {
        self.add_child(label, view);
        self
    }

    pub fn child_conditional(mut self, condition: bool, label: &str, view: impl View) -> Self {
        if condition {
            self.add_child(label, view);
        }
        self
    }

    pub fn get_child<T: View>(&self, index: usize) -> Option<&T> {
        self.view
            .get_child(1)?
            .downcast_ref::<ResizedView<LinearLayout>>()?
            .get_inner()
            .get_child(index)?
            .downcast_ref::<T>()
    }

    pub fn get_child_with_data<T: View>(&self, index: usize) -> Option<(&T, &UDT)> {
        let view = self
            .view
            .get_child(1)?
            .downcast_ref::<ResizedView<LinearLayout>>()?
            .get_inner()
            .get_child(index)?
            .downcast_ref::<T>()?;

        let data = self.user_data.get(&index)?;
        Some((view, data))
    }

    pub fn get_child_mut<T: View>(&mut self, index: usize) -> Option<&mut T> {
        self.view
            .get_child_mut(1)?
            .downcast_mut::<ResizedView<LinearLayout>>()?
            .get_inner_mut()
            .get_child_mut(index)?
            .downcast_mut::<T>()
    }

    pub fn get_value<T, R>(&self, index: usize) -> Option<R>
    where
        T: View + FormViewGetValue<R>,
    {
        self.get_child::<T>(index)?.get_value()
    }

    pub fn replace_child(&mut self, index: usize, view: impl View) {
        let parent = self
            .view
            .get_child_mut(1)
            .and_then(|v| v.downcast_mut())
            .map(ResizedView::<LinearLayout>::get_inner_mut);

        if let Some(parent) = parent {
            parent.remove_child(index);
            parent.insert_child(index, view);
        }
    }

    pub fn call_on_childs<T: View>(&mut self, callback: &dyn Fn(&mut T)) {
        for i in 0..self.len() {
            if let Some(v) = self.get_child_mut::<T>(i) {
                callback(v);
            }
        }
    }

    pub fn set_label_enabled(&mut self, index: usize, enabled: bool) {
        let style = if enabled {
            StyleType::primary()
        } else {
            StyleType::secondary()
        };

        let label = self
            .view
            .get_child_mut(0)
            .and_then(|v| v.downcast_mut::<ResizedView<LinearLayout>>())
            .and_then(|v| v.get_inner_mut().get_child_mut(index))
            .and_then(|v| v.downcast_mut::<TextView>());

        if let Some(label) = label {
            label.set_style(style);
        }
    }

    pub fn set_labels_enabled(&mut self, enabled: bool) {
        for i in 0..self.len() {
            self.set_label_enabled(i, enabled);
        }
    }

    pub fn len(&self) -> usize {
        self.view
            .get_child(1)
            .and_then(|v| v.downcast_ref::<ResizedView<LinearLayout>>())
            .unwrap()
            .get_inner()
            .len()
    }

    fn add_to_column(&mut self, index: usize, view: impl View) {
        self.view
            .get_child_mut(index)
            .and_then(|v| v.downcast_mut::<ResizedView<LinearLayout>>())
            .unwrap()
            .get_inner_mut()
            .add_child(view);
    }
}

impl<UDT: Send + Sync + 'static> ViewWrapper for FormView<UDT> {
    cursive::wrap_impl!(self.view: LinearLayout);

    fn wrap_important_area(&self, size: Vec2) -> Rect {
        // This fixes scrolling on small screen when many elements are present, e.g. bootdisk/RAID
        // list. Without this, scrolling completely down and then back up would not properly
        // display the currently selected form element.
        // tl;dr: For whatever reason, the inner `LinearLayout` calculates the rect with a line
        // height of 2. So e.g. if the first form element is selected, the y-coordinate is 2, if
        // the second is selected it is 4 and so on. Knowing that, this can fortunately be quite
        // easy fixed by just dividing the y-coordinate by 2 and adjusting the size of the area
        // rectanglo to 1.

        let inner = self.view.important_area(size);
        let top_left = inner.top_left().map_y(|y| y / 2);

        Rect::from_size(top_left, (inner.width(), 1))
    }
}

pub struct CidrAddressEditView {
    view: LinearLayout,
}

impl CidrAddressEditView {
    pub fn new() -> Self {
        let view = LinearLayout::horizontal()
            .child(EditView::new().full_width())
            .child(TextView::new(" / "))
            .child(Self::mask_edit_view(0));

        Self { view }
    }

    pub fn content(mut self, cidr: Cidr) -> Self {
        self.set_content(cidr);
        self
    }

    pub fn set_content(&mut self, cidr: Cidr) {
        if let Some(view) = self
            .view
            .get_child_mut(0)
            .and_then(|v| v.downcast_mut::<ResizedView<EditView>>())
        {
            *view = EditView::new()
                .content(cidr.address().to_string())
                .full_width();
        }

        if let Some(view) = self
            .view
            .get_child_mut(2)
            .and_then(|v| v.downcast_mut::<ResizedView<IntegerEditView>>())
        {
            *view = Self::mask_edit_view(cidr.mask());
        }
    }

    fn mask_edit_view(content: u8) -> ResizedView<IntegerEditView> {
        IntegerEditView::new()
            .max_value(128)
            .max_content_width(3)
            .content(content.into())
            .fixed_width(4)
    }

    fn get_values(&self) -> Option<(IpAddr, u8)> {
        let addr = self
            .view
            .get_child(0)?
            .downcast_ref::<ResizedView<EditView>>()?
            .get_inner()
            .get_content()
            .parse::<IpAddr>()
            .ok()?;

        let mask = self
            .view
            .get_child(2)?
            .downcast_ref::<ResizedView<IntegerEditView>>()?
            .get_inner()
            .get_content()
            .ok()?;

        Some((addr, mask as u8))
    }
}

impl ViewWrapper for CidrAddressEditView {
    cursive::wrap_impl!(self.view: LinearLayout);
}

/// List of checkboxes, each with an associated label and user-defined data for identifying its
/// meaning.
pub struct CheckboxGroup<UDT = ()> {
    view: LinearLayout,
    user_data: Vec<UDT>,
}

impl<UDT> CheckboxGroup<UDT> {
    pub fn new() -> Self {
        Self {
            view: LinearLayout::vertical(),
            user_data: Vec::new(),
        }
    }

    pub fn add(&mut self, label: &str, checked: bool, data: UDT) {
        self.user_data.push(data);
        self.view.add_child(
            LinearLayout::horizontal()
                .child(Checkbox::new().with_checked(checked))
                .child(DummyView)
                .child(TextView::new(label)),
        );
    }

    /// Returns whether each checkbox is checked, along with its data.
    pub fn entries(&self) -> impl Iterator<Item = (bool, &UDT)> {
        self.user_data.iter().enumerate().map(|(i, data)| {
            let checked = self
                .view
                .get_child(i)
                .and_then(|v| v.downcast_ref::<LinearLayout>())
                .and_then(|row| row.get_child(0)?.downcast_ref::<Checkbox>())
                .is_some_and(Checkbox::is_checked);

            (checked, data)
        })
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        let label_style = if enabled {
            StyleType::primary()
        } else {
            StyleType::secondary()
        };

        for i in 0..self.view.len() {
            if let Some(label) = self
                .view
                .get_child_mut(i)
                .and_then(|v| v.downcast_mut::<LinearLayout>())
                .and_then(|v| v.get_child_mut(2))
                .and_then(|v| v.downcast_mut::<TextView>())
            {
                label.set_style(label_style);
            }

            if let Some(checkbox) = self
                .view
                .get_child_mut(i)
                .and_then(|v| v.downcast_mut::<LinearLayout>())
                .and_then(|v| v.get_child_mut(0))
                .and_then(|v| v.downcast_mut::<Checkbox>())
            {
                checkbox.set_enabled(enabled);
            }
        }
    }

    /// Sets the label of each checkbox to the one returned for its data.
    pub fn update_labels(&mut self, label_for: impl Fn(&UDT) -> String) {
        for (i, data) in self.user_data.iter().enumerate() {
            if let Some(label) = self
                .view
                .get_child_mut(i)
                .and_then(|v| v.downcast_mut::<LinearLayout>())
                .and_then(|row| row.get_child_mut(2)?.downcast_mut::<TextView>())
            {
                label.set_content(label_for(data));
            }
        }
    }
}

impl<UDT: Send + Sync + 'static> ViewWrapper for CheckboxGroup<UDT> {
    cursive::wrap_impl!(self.view: LinearLayout);
}
