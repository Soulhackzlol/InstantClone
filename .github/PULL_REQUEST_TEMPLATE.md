<!--
Thanks for sending a PR! A few notes that make review faster.

If this PR fixes an open issue, write "Closes #N" anywhere in the body
so GitHub auto-closes the issue on merge.

If it's behaviour-changing (not just a refactor or a doc tweak), the
"How I tested" section is the load-bearing one. "Tested with OBS 32.1.2
against Twitch ingest for ~5 min with a 15 s armed delay, both cuts
were clean" beats "works on my machine" because I can't tell which
encoder you used or what platform you streamed to otherwise.
-->

## What this does

<!-- One or two sentences. Why the change matters, not just the diff. -->

## How I tested

<!--
Whichever of these apply, just delete the rest:
- [ ] `cargo fmt --all -- --check` clean
- [ ] `cargo clippy --release --all-targets -- -D warnings` clean
- [ ] `cargo test --release` shows the expected pass count
- [ ] Manual: streamed through it for N minutes with target=<delay> s,
      destination=<twitch/youtube/kick/custom>, encoder=<NVENC/x264>
- [ ] Manual: <other scenario>
-->

## Notes for the reviewer

<!--
Anything that would otherwise come up as a review question.
Trade-offs you considered, things you tried that didn't work,
follow-ups deliberately scoped out, etc. Skip if the diff speaks
for itself.
-->
