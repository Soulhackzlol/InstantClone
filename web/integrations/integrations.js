// Integrations tab: the module list, the catalog, the module editor, the
// step builder, connections (Discord, Twitch, phone) and recipes.
//
// Loaded after the dashboard script, so it reuses its helpers ($, toast,
// staggerIn, fitPaneCard, flashCopied, prefersReducedMotion) and its
// components: a module is a .dcard, every editor is the destination modal
// (.ic-modal + .dest-form). All user text goes through esc() before it
// touches the DOM, and every redraw goes through morph() (see Rendering).
(function(){
'use strict';
// Run order, as on the dashboard: functions can live in any piece, what
// runs at load runs top to bottom, and init.js comes last.

<!-- include helpers.js -->
<!-- include alert-tiers.js -->
<!-- include labels.js -->
<!-- include values.js -->
<!-- include templates.js -->
<!-- include value-picker.js -->
<!-- include rendering.js -->
<!-- include motion.js -->
<!-- include controls.js -->
<!-- include web-request.js -->
<!-- include menu.js -->
<!-- include clipboard.js -->
<!-- include previews.js -->
<!-- include state.js -->
<!-- include pane.js -->
<!-- include folders.js -->
<!-- include actions.js -->
<!-- include modal.js -->
<!-- include catalog.js -->
<!-- include module-editor.js -->
<!-- include lights.js -->
<!-- include trigger-panels.js -->
<!-- include builder.js -->
<!-- include connections.js -->
<!-- include alerts-page.js -->
<!-- include alert-looks.js -->
<!-- include recipes.js -->
<!-- include on-stream.js -->
<!-- include folder-drag.js -->
<!-- include page-events.js -->
<!-- include init.js -->
})();
