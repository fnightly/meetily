  const consoleLines = [];
// Render the Meetily interface in a real browser with Tauri's IPC stubbed out.
//
// The pages are client-rendered, and every panel fetches through `invoke`, which delegates
// to `window.__TAURI_INTERNALS__.invoke`. Replacing that before the page loads lets the
// real components render their real markup against data of the right shape — which is the
// closest thing to looking at the window that can be done without the window.
//
// Usage: node cdpshot.cjs <url> <out.png> [settleMs]

const WebSocket = require('ws');
const http = require('http');
const fs = require('fs');

const [url, outPath, settleMs = '3500'] = process.argv.slice(2);

const getJson = (path) =>
  new Promise((resolve, reject) => {
    http.get({ host: '127.0.0.1', port: 9222, path }, (res) => {
      let body = '';
      res.on('data', (chunk) => (body += chunk));
      res.on('end', () => {
        try {
          resolve(JSON.parse(body));
        } catch (error) {
          reject(error);
        }
      });
    }).on('error', reject);
  });

// One labelled speaker run, so the grouping rule is visible in the capture: speaker 1
// speaks twice in a row and the label should appear once, then speaker 2.
const TRANSCRIPTS = [
  { id: 't1', text: '大家好，我们今天主要讨论三件事。', timestamp: '00:00', audio_start_time: 1.2, audio_end_time: 4.0, speaker: 'speaker_1' },
  { id: 't2', text: '第一件是下个季度的排期。', timestamp: '00:04', audio_start_time: 4.2, audio_end_time: 7.5, speaker: 'speaker_1' },
  { id: 't3', text: '我这边需要先确认一下人力。', timestamp: '00:08', audio_start_time: 8.0, audio_end_time: 11.4, speaker: 'speaker_2' },
  { id: 't4', text: '可以，我周五之前给你答复。', timestamp: '00:12', audio_start_time: 12.0, audio_end_time: 15.0, speaker: 'speaker_1' },
  { id: 't5', text: '那我们进入第二件事。', timestamp: '00:16', audio_start_time: 16.0, audio_end_time: 18.5, speaker: null },
];

const MEETING = {
  id: 'meeting-demo',
  title: '产品周会',
  created_at: '2026-10-15T13:12:54Z',
  updated_at: '2026-10-15T13:20:00Z',
  folder_path: 'D:\\meetily-recordings\\Meeting demo',
  transcripts: TRANSCRIPTS,
  aiSummary: null,
};

const STUB = `
window.__TAURI_INTERNALS__ = {
  transformCallback: function () { return 1; },
  unregisterCallback: function () {},
  convertFileSrc: function (p) { return p; },
  invoke: async function (cmd, args) {
    const T = ${JSON.stringify(TRANSCRIPTS)};
    const M = ${JSON.stringify(MEETING)};

    // Tauri plugins use the same IPC channel. Their shapes matter: a store read is
    // destructured as a [value, exists] pair, and an event listen returns an id.
    if (cmd === 'plugin:event|listen') return 1;
    if (cmd === 'plugin:event|unlisten') return null;
    if (cmd === 'plugin:event|emit') return null;
    if (cmd === 'plugin:event|emit_to') return null;
    if (cmd === 'plugin:store|load') return 1;
    if (cmd === 'plugin:store|get') return [null, false];
    if (cmd === 'plugin:store|set') return null;
    if (cmd === 'plugin:store|save') return null;
    if (cmd === 'plugin:store|delete') return null;
    if (cmd === 'plugin:store|entries') return [];
    if (cmd === 'plugin:store|keys') return [];
    if (cmd === 'plugin:store|values') return [];
    if (cmd === 'plugin:store|clear') return null;
    if (cmd === 'plugin:store|reset') return null;
    if (cmd === 'plugin:store|has') return false;
    if (cmd === 'plugin:store|length') return 0;
    if (cmd && cmd.indexOf('plugin:') === 0) return null;

    // Commands whose result is walked as a collection. Returning null for these makes the
    // application's own guards untested and its render tree collapse, which would say
    // nothing about the interface.
    if (cmd.indexOf('list') !== -1 || cmd.indexOf('templates') !== -1) return [];
    if (/^(get_ollama_models|api_get_dictionary)/.test(cmd)) return [];

    const table = {
      api_get_meeting_transcripts: function () {
        return { transcripts: T, total_count: T.length, has_more: false, loaded_count: T.length };
      },
      api_get_meeting: function () { return M; },
      api_get_meeting_metadata: function () {
        // folder_path matters: the meeting view reads it to decide whether the
        // folder-bound actions (identify speakers, re-transcribe) can be offered.
        return {
          id: M.id,
          title: M.title,
          created_at: M.created_at,
          updated_at: M.updated_at,
          folder_path: M.folder_path,
        };
      },
      api_get_meetings: function () { return [M]; },
      get_diarization_model_status: function () {
        return {
          ready: true,
          folder: 'D:\\\\Meetly\\\\Model\\\\diarization',
          missing: [],
          message: 'Models ready',
          required_bytes: 0,
          available_bytes: 12000000000,
          has_room: true,
        };
      },
      get_diarization_preferences: function () {
        return {
          preferences: { threshold: 0.75, max_speakers: 12 },
          bounds: {
            min_threshold: 0.4, max_threshold: 0.95,
            min_speakers: 2, max_speakers: 32,
            calibrated_threshold: 0.75,
          },
          is_default: true,
        };
      },
      api_get_transcript_config: function () {
        return { provider: 'localWhisper', model: 'tiny', language: 'zh' };
      },
      api_list_speech_providers: function () {
        return [
          {
            id: 'localWhisper', display_name: 'Whisper (本地)', kind: 'local', status: 'available',
            supported_languages: null, requires_api_key: false, has_api_key: false,
            models: ['tiny', 'base', 'small'], notes: '', api_key_url: null, selectable: true,
          },
          {
            id: 'parakeet', display_name: 'Parakeet', kind: 'local', status: 'available',
            supported_languages: ['en'], requires_api_key: false, has_api_key: false,
            models: ['parakeet-tdt-0.6b-v2'], notes: '', api_key_url: null, selectable: true,
          },
        ];
      },
      api_get_active_speech_settings: function () {
        return { provider: 'localWhisper', model: 'tiny', language: 'zh', has_api_key: false };
      },
      api_get_provider_languages: function () { return null; },
      api_validate_speech_selection: function () { return { valid: true, message: null }; },
      api_get_model_config: function () {
        return { provider: 'builtin-ai', model: 'qwen3.5:4b', whisperModel: 'tiny' };
      },
      api_get_api_key: function () { return ''; },
      get_storage_config: function () {
        return {
          config: { database_folder: 'D:\\\\Meetly\\\\data', models_folder: 'D:\\\\Meetly\\\\Model', notes_folder: 'E:\\\\notes', recordings_folder: 'D:\\\\meetily-recordings' },
          resolved: { database_folder: 'D:\\\\Meetly\\\\data', database_file: 'D:\\\\Meetly\\\\data\\\\meeting_minutes.sqlite', models_folder: 'D:\\\\Meetly\\\\Model', notes_folder: 'E:\\\\notes', recordings_folder: 'D:\\\\meetily-recordings', app_data_dir: 'C:\\\\Users\\\\Administrator\\\\AppData\\\\Roaming\\\\com.meetily.ai' },
        };
      },
      get_recording_preferences: function () {
        return { save_folder: 'D:\\\\meetily-recordings', auto_save: true, file_format: 'mp3', quality: 'high', defer_transcription: true, cleanup_after_recording: false };
      },
      get_onboarding_status: function () {
        return {
          step: 4,
          completed: true,
          model_status: { parakeet: 'downloaded', whisper: 'downloaded', summary: 'downloaded' },
        };
      },
      get_default_recordings_folder_path: function () { return 'D:\\\\meetily-recordings'; },
      get_recording_state: function () {
        return {
          is_recording: false,
          is_paused: false,
          is_active: false,
          recording_duration: null,
          active_duration: null,
        };
      },
      parakeet_init: function () { return null; },
      api_get_available_models: function () { return []; },
      get_available_audio_backends: function () { return ['wasapi']; },
      get_current_audio_backend: function () { return 'wasapi'; },
      api_get_dictionary: function () { return []; },
      get_summary_templates: function () { return []; },
      list_meeting_notes: function () { return []; },
      get_meeting_note: function () { return null; },
      list_meetings: function () { return [M]; },
    };
    if (table[cmd]) return table[cmd]();
    console.log('[stub] unhandled command:', cmd);
    return null;
  },
};
`;

(async () => {
  const targets = await getJson('/json/list');
  const page = targets.find((t) => t.type === 'page');
  if (!page) throw new Error('no page target');

  const ws = new WebSocket(page.webSocketDebuggerUrl, { maxPayload: 256 * 1024 * 1024 });
  let id = 0;
  const waiting = new Map();

  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const messageId = ++id;
      waiting.set(messageId, { resolve, reject });
      ws.send(JSON.stringify({ id: messageId, method, params }));
    });

  ws.on('message', (raw) => {
    const message = JSON.parse(raw.toString());
    if (message.method === 'Runtime.consoleAPICalled') {
      const text = (message.params.args || [])
        .map((arg) => arg.value ?? arg.description ?? '')
        .join(' ');
      consoleLines.push(`[${message.params.type}] ${text}`);
    }
    if (message.id && waiting.has(message.id)) {
      const { resolve, reject } = waiting.get(message.id);
      waiting.delete(message.id);
      if (message.error) reject(new Error(JSON.stringify(message.error)));
      else resolve(message.result);
    }
  });

  await new Promise((resolve) => ws.on('open', resolve));
  await send('Page.enable');
  await send('Runtime.enable');

  // Must run before the app's bundle, so the stub is in place when React mounts.
  await send('Page.addScriptToEvaluateOnNewDocument', { source: STUB });

  // Several panels are taller than the default viewport; a full-height capture is the
  // difference between seeing a section and seeing only its heading.
  await send('Emulation.setDeviceMetricsOverride', {
    width: 1280,
    height: Number(process.argv[6] || 2600),
    deviceScaleFactor: 1,
    mobile: false,
  });

  await send('Page.navigate', { url });
  await new Promise((resolve) => setTimeout(resolve, Number(settleMs)));

  // Optionally click a tab or button by its visible text, so panels behind a tab can be
  // captured. The app's tabs only mount their content once selected.
  const clickText = process.argv[5];
  if (clickText) {
    const clicked = await send('Runtime.evaluate', {
      expression: `(() => {
        const wanted = ${JSON.stringify(clickText)};
        const nodes = Array.from(document.querySelectorAll('button,[role="tab"],a'));
        const hit = nodes.find(n => (n.textContent || '').trim() === wanted)
                 || nodes.find(n => (n.textContent || '').includes(wanted));
        if (!hit) return 'not found';
        // Radix responds to pointer events, not a bare click().
        const opts = { bubbles: true, cancelable: true, composed: true, button: 0 };
        hit.dispatchEvent(new PointerEvent('pointerdown', opts));
        hit.dispatchEvent(new MouseEvent('mousedown', opts));
        hit.dispatchEvent(new PointerEvent('pointerup', opts));
        hit.dispatchEvent(new MouseEvent('mouseup', opts));
        hit.click();
        return 'clicked: ' + (hit.textContent || '').trim();
      })()`,
      returnByValue: true,
    });
    console.log('click ->', clicked.result && clicked.result.value);
    await new Promise((resolve) => setTimeout(resolve, 2500));
  }

  const errors = await send('Runtime.evaluate', {
    expression: 'JSON.stringify(window.__capturedErrors || [])',
    returnByValue: true,
  });

  const shot = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true });
  fs.writeFileSync(outPath, Buffer.from(shot.data, 'base64'));

  const text = await send('Runtime.evaluate', {
    expression: 'document.body.innerText.slice(0, 1200)',
    returnByValue: true,
  });

  console.log(JSON.stringify({
    saved: outPath,
    bytes: fs.statSync(outPath).size,
    errors: errors.result && errors.result.value,
    console: consoleLines.slice(-25),
    visibleText: text.result && text.result.value,
  }, null, 2));

  ws.close();
})().catch((error) => {
  console.error('FAILED:', error.message);
  process.exit(1);
});
