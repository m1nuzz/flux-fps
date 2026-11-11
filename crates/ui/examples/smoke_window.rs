//! Minimal windows-reactor window used by the CI UI smoke gate.
//!
//! Purpose: bisect "no visible window" failures. If this tiny counter window
//! becomes visible while `flux-fps.exe` does not, the defect is in the
//! resident app's view tree or startup path, not in the environment. Build it
//! with `cargo build -p flux-fps --example smoke_window`.

use windows_reactor::{
    App, Button, ChildrenControl, Component, ComponentContext, ContentControl, StackPanel,
    TextBlock, View, ViewContext,
};

struct Counter {
    count: u32,
}

impl Component for Counter {
    type Input = ();
    type Message = ();

    fn create(_input: &(), _context: &ComponentContext<Self>) -> Self {
        Self { count: 0 }
    }

    fn update(&mut self, _message: (), _context: &ComponentContext<Self>) {
        self.count += 1;
    }

    fn view(&self, _input: &(), context: &mut ViewContext<Self>) -> View {
        StackPanel::new().spacing(8.0).children((
            TextBlock::new().text(format!("flux-fps smoke window, count: {}", self.count)),
            Button::new()
                .on_click(context.forward())
                .content("Increment"),
        ))
    }
}

fn main() {
    App::run_component::<Counter>(()).unwrap();
}
