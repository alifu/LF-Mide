//! The build variant (Android) or scheme (iOS) picker.

use crate::devices::Platform;
use crate::output::Job;
use crate::state::{AppState, VariantPicker};

fn say(state: AppState, message: impl Into<String>) -> AppState {
    AppState {
        status: Some(message.into()),
        ..state
    }
}

/// Opens the picker for the current target and asks for its choices.
pub(crate) fn open(state: AppState) -> AppState {
    let platform = state.target;
    let mut pending_jobs = state.pending_jobs.clone();
    pending_jobs.push(Job::LoadVariants(platform));
    AppState {
        variant_picker: Some(VariantPicker {
            platform,
            options: None,
            highlight: 0,
        }),
        pending_jobs,
        ..state
    }
}

pub(crate) fn loaded(
    state: AppState,
    platform: Platform,
    result: Result<Vec<String>, String>,
) -> AppState {
    let Some(picker) = state.variant_picker.clone() else {
        return state;
    };
    if picker.platform != platform {
        return state;
    }
    let closed = AppState {
        variant_picker: None,
        ..state.clone()
    };
    match result {
        Err(reason) => say(closed, format!("No {}: {reason}", choice_noun(platform))),
        Ok(options) if options.is_empty() => say(
            closed,
            format!("No {} found in the project", choice_noun(platform)),
        ),
        Ok(options) => {
            let current = state.variants.get(&platform);
            let highlight = options
                .iter()
                .position(|option| Some(option) == current)
                .unwrap_or(0);
            AppState {
                variant_picker: Some(VariantPicker {
                    platform,
                    options: Some(options),
                    highlight,
                }),
                ..state
            }
        }
    }
}

pub(crate) fn step(state: AppState, delta: i16) -> AppState {
    let Some(picker) = &state.variant_picker else {
        return state;
    };
    let Some(options) = &picker.options else {
        return state;
    };
    let rows = options.len() as i32;
    let highlight = (picker.highlight as i32 + i32::from(delta)).rem_euclid(rows) as usize;
    AppState {
        variant_picker: Some(VariantPicker {
            highlight,
            ..picker.clone()
        }),
        ..state
    }
}

pub(crate) fn select(state: AppState) -> AppState {
    let Some(picker) = &state.variant_picker else {
        return state;
    };
    let Some(choice) = picker
        .options
        .as_ref()
        .and_then(|options| options.get(picker.highlight))
        .cloned()
    else {
        return state;
    };
    let mut variants = state.variants.clone();
    variants.insert(picker.platform, choice.clone());
    let message = format!("Build {}: {choice}", choice_noun(picker.platform));
    say(
        AppState {
            variants,
            variant_picker: None,
            ..state
        },
        message,
    )
}

pub(crate) fn cancel(state: AppState) -> AppState {
    AppState {
        variant_picker: None,
        ..state
    }
}

/// What the platform calls its choice.
pub fn choice_noun(platform: Platform) -> &'static str {
    match platform {
        Platform::Android => "variant",
        Platform::Ios => "scheme",
    }
}

#[cfg(test)]
mod tests {
    use crate::action::Action;
    use crate::reducer::reduce;
    use crate::state::AppState;
    use crate::{Config, Project, ProjectLayout};

    use super::*;

    fn state() -> AppState {
        AppState::new(
            Project {
                root: "/p".into(),
                name: "demo".into(),
                layout: ProjectLayout::ComposeApp,
            },
            Config::default(),
        )
    }

    fn options(names: &[&str]) -> Action {
        Action::VariantsLoaded {
            platform: Platform::Android,
            result: Ok(names.iter().map(ToString::to_string).collect()),
        }
    }

    #[test]
    fn opening_asks_for_the_choices_of_the_current_target() {
        let s = reduce(state(), Action::OpenVariantPicker);

        assert!(matches!(
            s.pending_jobs.last(),
            Some(Job::LoadVariants(Platform::Android))
        ));
        assert_eq!(s.variant_picker.unwrap().options, None);
    }

    #[test]
    fn the_current_choice_starts_highlighted_and_selecting_remembers_a_new_one() {
        let mut s = state();
        s.variants.insert(Platform::Android, "freeDebug".into());
        let s = reduce(s, Action::OpenVariantPicker);
        let s = reduce(s, options(&["debug", "freeDebug", "paidDebug"]));
        assert_eq!(s.variant_picker.as_ref().unwrap().highlight, 1);

        let s = reduce(s, Action::VariantPickerMove(1));
        let s = reduce(s, Action::VariantPickerSelect);

        assert_eq!(s.variants[&Platform::Android], "paidDebug");
        assert_eq!(s.variant_picker, None);
        assert!(s.status.unwrap().contains("paidDebug"));
    }

    #[test]
    fn moving_wraps_around() {
        let s = reduce(state(), Action::OpenVariantPicker);
        let s = reduce(s, options(&["a", "b"]));

        let s = reduce(s, Action::VariantPickerMove(-1));

        assert_eq!(s.variant_picker.unwrap().highlight, 1);
    }

    #[test]
    fn no_choices_or_an_error_closes_the_picker_with_a_reason() {
        let open = reduce(state(), Action::OpenVariantPicker);

        let empty = reduce(open.clone(), options(&[]));
        let failed = reduce(
            open,
            Action::VariantsLoaded {
                platform: Platform::Android,
                result: Err("no build file".into()),
            },
        );

        assert_eq!(empty.variant_picker, None);
        assert!(empty.status.unwrap().contains("No variant"));
        assert!(failed.status.unwrap().contains("no build file"));
    }

    #[test]
    fn a_late_answer_for_another_platform_or_a_closed_picker_is_ignored() {
        let open = reduce(state(), Action::OpenVariantPicker);
        let other = reduce(
            open.clone(),
            Action::VariantsLoaded {
                platform: Platform::Ios,
                result: Ok(vec!["x".into()]),
            },
        );
        let closed = reduce(reduce(open, Action::VariantPickerCancel), options(&["a"]));

        assert_eq!(other.variant_picker.unwrap().options, None);
        assert_eq!(closed.variant_picker, None);
    }

    #[test]
    fn a_build_uses_the_chosen_variant_of_its_platform() {
        let mut s = state();
        s.variants.insert(Platform::Ios, "iosApp-Staging".into());
        s.variants.insert(Platform::Android, "freeDebug".into());
        let device = crate::Device {
            platform: Platform::Android,
            id: "Pixel_8".into(),
            name: "Pixel 8".into(),
            detail: String::new(),
            state: crate::DeviceState::Running,
            serial: Some("emulator-5554".into()),
        };
        s.devices = std::sync::Arc::new(vec![device]);
        s.selected_devices
            .insert(Platform::Android, "Pixel_8".into());

        let s = reduce(s, Action::RunApp);

        let Some(Job::Build(request)) = s.pending_jobs.last() else {
            panic!("no build job")
        };
        assert_eq!(request.variant.as_deref(), Some("freeDebug"));
    }
}
