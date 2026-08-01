-- InstantClone VOD track
--
-- OBS hardcodes the "Twitch VOD Track" feature to the service literally
-- named "Twitch" (frontend gate `ServiceSupportsVodTrack` = {"Twitch"}),
-- so it's unavailable while using the InstantClone service. This script
-- does what OBS's own VOD setup does - attach a second audio encoder to
-- the stream output at audio index 1 - but WITHOUT the service gate. OBS
-- then sends two audio tracks; InstantClone forwards both to Twitch, and
-- Twitch archives wire TrackId 1 as the VOD.
--
-- Route the audio you want in the VOD (e.g. music the live mix mutes) to
-- an OBS audio track via Advanced Audio Properties, then pick that track
-- below. Keep the live stream on a different track (usually track 1).
-- Requires Output Mode: Advanced. Track/bitrate changes apply on your
-- next stream.
--
-- OWNERSHIP (this is the whole ballgame, verified against libobs source):
-- `obs_output_set_audio_encoder` takes its OWN reference to the encoder
-- (`output->audio_encoders[idx] = obs_encoder_get_ref(encoder)`), and the
-- output releases it when it is destroyed. So the correct pattern - the
-- one OBS's own AdvancedOutput uses - is: create the encoder, attach it,
-- then release OUR create-reference IMMEDIATELY. After that the OUTPUT
-- solely owns the encoder and manages its whole lifecycle; we never touch
-- it again. Holding our reference and releasing it later (on stop, on the
-- next start, on unload) is exactly what raced OBS and crashed
-- (c0000005 in receive_audio / obs_encoder_release). So: no encoder state
-- kept, no deferred releases, nothing to clean up.

local obs = obslua

local ENCODER_ID = "ffmpeg_aac" -- always-present AAC encoder
local ENCODER_NAME = "InstantClone VOD audio" -- how we recognise our own track

-- ── settings (mirrored from the script properties UI) ────────────────
local enabled = true
local vod_track = 2 -- OBS mixer track (2..6) that carries the VOD audio
local vod_bitrate = 160 -- kbps
local verbose = false -- extra diagnostic logging (off for clean logs)

local function log(msg)
    obs.script_log(obs.LOG_INFO, "[InstantClone VOD] " .. msg)
end

local function vlog(msg)
    if verbose then
        log(msg)
    end
end

-- Inspect output audio index 1. Returns (present, ours). The getter is
-- non-owning, so we don't release what it returns.
local function inspect_index_1(output)
    local existing = obs.obs_output_get_audio_encoder(output, 1)
    if existing == nil then
        return false, false
    end
    return true, obs.obs_encoder_get_name(existing) == ENCODER_NAME
end

-- Called on STREAMING_STARTING - the pre-start window OBS uses for its own
-- native VOD setup, so the encoder is picked up when the output starts.
local function on_streaming_starting()
    local output = obs.obs_frontend_get_streaming_output()
    if output == nil then
        log("no streaming output at start - cannot attach VOD track")
        return
    end

    local present, ours = inspect_index_1(output)

    -- Disabled: remove our track if it's ours (never touch a real Twitch
    -- native VOD track). Setting nil drops the output's reference for us.
    if not enabled then
        if present and ours then
            obs.obs_output_set_audio_encoder(output, nil, 1)
        end
        obs.obs_output_release(output)
        return
    end

    -- Don't clobber OBS's own native VOD track (a real Twitch service).
    if present and not ours then
        log("audio index 1 is OBS's native VOD track - leaving it alone")
        obs.obs_output_release(output)
        return
    end

    -- Create a fresh encoder, attach it, and immediately release our
    -- create-reference. The output now owns it (and releases the previous
    -- index-1 encoder as part of the set). We keep no reference and never
    -- release it again - see the OWNERSHIP note.
    local settings = obs.obs_data_create()
    obs.obs_data_set_int(settings, "bitrate", vod_bitrate)
    local enc = obs.obs_audio_encoder_create(ENCODER_ID, ENCODER_NAME, settings, vod_track - 1, nil)
    obs.obs_data_release(settings)
    if enc == nil then
        log("failed to create VOD audio encoder")
        obs.obs_output_release(output)
        return
    end
    obs.obs_encoder_set_audio(enc, obs.obs_get_audio())
    obs.obs_output_set_audio_encoder(output, enc, 1)
    obs.obs_encoder_release(enc) -- balance our create-ref; the output owns it now
    log(string.format("VOD track attached: OBS track %d -> stream audio 2 @ %d kbps", vod_track, vod_bitrate))
    obs.obs_output_release(output)
end

-- Diagnostic only (verbose): confirm our track is at index 1 after start.
local function verify_after_start()
    if not verbose then
        return
    end
    local output = obs.obs_frontend_get_streaming_output()
    if output == nil then
        return
    end
    local present, ours = inspect_index_1(output)
    vlog(string.format("post-start check: idx1 present=%s ours=%s", tostring(present), tostring(ours)))
    obs.obs_output_release(output)
end

local function on_event(event)
    if event == obs.OBS_FRONTEND_EVENT_STREAMING_STARTING then
        on_streaming_starting()
    elseif event == obs.OBS_FRONTEND_EVENT_STREAMING_STARTED then
        verify_after_start()
    end
    -- No STREAMING_STOPPED handler and no encoder release anywhere else -
    -- the output owns the encoder (see the OWNERSHIP note).
end

-- ── OBS script entry points ──────────────────────────────────────────
function script_description()
    return [[<b>InstantClone VOD track</b><br/><br/>
Sends a second audio track (the VOD / archive track) on the stream for
<i>any</i> service, so the Twitch VOD audio works while using the
InstantClone service - OBS's built-in VOD track is locked to the Twitch
service.<br/><br/>
Route the audio you want in the VOD to an OBS audio track in
<b>Advanced Audio Properties</b>, then pick that track below. Keep the
live stream on a different track (usually track 1). Requires
<b>Output Mode: Advanced</b>. Track / bitrate changes apply on your next
stream.]]
end

function script_properties()
    local props = obs.obs_properties_create()
    obs.obs_properties_add_bool(props, "enabled", "Enable VOD track")
    obs.obs_properties_add_int(props, "vod_track", "VOD audio track (OBS mixer track 2-6)", 2, 6, 1)
    obs.obs_properties_add_int(props, "vod_bitrate", "VOD audio bitrate (kbps)", 64, 320, 8)
    obs.obs_properties_add_bool(props, "verbose", "Verbose logging (debug)")
    return props
end

function script_defaults(settings)
    obs.obs_data_set_default_bool(settings, "enabled", true)
    obs.obs_data_set_default_int(settings, "vod_track", 2)
    obs.obs_data_set_default_int(settings, "vod_bitrate", 160)
    obs.obs_data_set_default_bool(settings, "verbose", false)
end

function script_update(settings)
    -- Just record values - the next stream's fresh encoder picks them up.
    -- Clamp defensively in case a hand-edited settings file is out of range.
    enabled = obs.obs_data_get_bool(settings, "enabled")
    verbose = obs.obs_data_get_bool(settings, "verbose")
    vod_track = math.max(2, math.min(6, obs.obs_data_get_int(settings, "vod_track")))
    vod_bitrate = math.max(64, math.min(320, obs.obs_data_get_int(settings, "vod_bitrate")))
end

function script_load(settings)
    obs.obs_frontend_add_event_callback(on_event)
    log("loaded - will attach VOD track on stream start")
end

function script_unload()
    -- We hold no encoder references, so there is nothing to clean up but the
    -- event callback (removing it keeps a reload from stacking handlers).
    obs.obs_frontend_remove_event_callback(on_event)
end
