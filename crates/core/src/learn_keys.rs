use crate::keymap::{KeyCommand, LearnedBinding};
use crate::*;

impl AppState {
    pub fn resolve_key_command<'a>(
        &'a self,
        base: &'a Keymap,
        context: KeyContext,
        chord: KeyChord,
    ) -> Option<&'a KeyCommand> {
        self.learned_keymap
            .resolve(context, chord)
            .or_else(|| base.resolve(context, chord))
    }

    pub(crate) fn refresh_learned_keymap(&mut self) {
        self.learned_keymap = Keymap::from_learned_bindings(&self.settings.learn_keys.bindings);
        let mut effective = self.base_keymap.clone();
        effective.merge_from(&self.learned_keymap);
        self.keybinding_hints = KeybindingHints::from_keymap(&effective);
    }

    pub(crate) fn open_learned_binding(
        &mut self,
        chord: KeyChord,
        original: Option<LearnedBinding>,
    ) {
        let initial = original.as_ref().or_else(|| {
            self.settings.learn_keys.bindings.iter().find(|binding| {
                binding.context == "filemanager"
                    && binding.parse().is_ok_and(|(_, bound, _)| bound == chord)
            })
        });
        let context = initial
            .map(|binding| binding.context.as_str())
            .unwrap_or("filemanager")
            .to_owned();
        let action = initial
            .map(|binding| binding.action.as_str())
            .unwrap_or("View")
            .to_owned();
        self.learned_binding_dialog(chord, original, context, action);
    }

    fn learned_binding_dialog(
        &mut self,
        chord: KeyChord,
        original: Option<LearnedBinding>,
        context: String,
        action: String,
    ) {
        self.push_dialog(
            DialogState::pair_input(
                format!("Bind {}", format_key_chord(chord)),
                "Context (filemanager, viewer, tree, ...):",
                context,
                "Command (View, Copy, Swap, ...; blank removes):",
                action,
            ),
            PendingDialogAction::LearnKey { chord, original },
        );
        self.set_status("Assign a command; Save setup persists learned bindings");
    }

    pub(crate) fn save_learned_binding(
        &mut self,
        chord: KeyChord,
        original: Option<LearnedBinding>,
        context: String,
        action: String,
    ) {
        if original
            .as_ref()
            .is_some_and(|original| !self.settings.learn_keys.bindings.contains(original))
        {
            self.set_status("Binding changed while the editor was open");
            return;
        }
        let binding = LearnedBinding {
            context: context.trim().to_ascii_lowercase(),
            action: action.trim().to_owned(),
            chord: chord.config_token(),
        };
        let remove = binding.action.is_empty();
        if !remove && let Err(error) = binding.parse() {
            self.learned_binding_dialog(chord, original, context, action);
            self.set_status(error);
            return;
        }
        let bindings = &mut self.settings.learn_keys.bindings;
        let replaces = |current: &LearnedBinding| {
            current.parse().is_ok_and(|(context, bound, _)| {
                context.section_name() == binding.context && bound == chord
            })
        };
        if !remove && bindings.len() >= 128 && !bindings.iter().any(replaces) && original.is_none()
        {
            self.set_status("Remove a learned binding before adding more (limit: 128)");
            return;
        }
        bindings.retain(|current| Some(current) != original.as_ref() && !replaces(current));
        if !remove {
            bindings.push(binding);
        }
        self.refresh_learned_keymap();
        self.settings.mark_dirty();
        self.refresh_settings_entries();
        self.set_status(if remove {
            "Learned binding removed; Save setup to persist"
        } else {
            "Binding applied; Save setup to persist"
        });
    }

    pub(crate) fn edit_learned_binding(&mut self, index: usize) {
        if let Some(binding) = self.settings.learn_keys.bindings.get(index).cloned()
            && let Ok((_, chord, _)) = binding.parse()
        {
            self.open_learned_binding(chord, Some(binding));
        }
    }
}
