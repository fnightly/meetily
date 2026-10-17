import { afterAll, afterEach, beforeEach, describe, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';

/**
 * The clipboard copy is one of the ways a transcript leaves the app, so whatever the
 * interface shows has to be in the pasted text too.
 */

const originalCore = { ...(await import('@tauri-apps/api/core')) };
const originalToast = { ...(await import('sonner')) };

type Pending = { command: string; args: Record<string, unknown>; resolve: (value: unknown) => void };
const pending: Pending[] = [];

const invoke = mock((command: string, args: Record<string, unknown> = {}) => new Promise((resolve) => {
  pending.push({ command, args, resolve });
}));

mock.module('@tauri-apps/api/core', () => ({ ...originalCore, invoke }));

const toast = { success: mock(() => undefined), error: mock(() => undefined), info: mock(() => undefined) };
mock.module('sonner', () => ({ ...originalToast, toast }));

const written: string[] = [];
// Bun provides `navigator` but not a clipboard; the hook only writes to it.
Object.defineProperty(globalThis.navigator, 'clipboard', {
  value: { writeText: async (text: string) => { written.push(text); } },
  configurable: true,
});

const { useCopyOperations } = await import('../../src/hooks/meeting-details/useCopyOperations');

afterAll(() => {
  mock.module('@tauri-apps/api/core', () => originalCore);
  mock.module('sonner', () => originalToast);
});

let renderer: ReactTestRenderer | undefined;
let api: ReturnType<typeof useCopyOperations>;

const meeting = {
  id: 'm1',
  title: 'A meeting',
  created_at: '2026-09-07T10:00:00Z',
};

function View() {
  api = useCopyOperations({
    meeting,
    transcripts: [],
    meetingTitle: 'A meeting',
    aiSummary: null,
    blockNoteSummaryRef: { current: null },
  });
  return null;
}

beforeEach(() => {
  pending.length = 0;
  written.length = 0;
  invoke.mockClear();
  toast.success.mockClear();
  toast.error.mockClear();
});

afterEach(async () => {
  if (renderer) await act(async () => renderer!.unmount());
  renderer = undefined;
});

async function copyWith(transcripts: unknown[], speakerNames: Record<string, string> = {}) {
  await act(async () => {
    renderer = create(<View />);
  });

  await act(async () => {
    void api.handleCopyTranscript();
    // The copy runs asynchronously and is deliberately not awaited (it waits on requests
    // this test resolves by hand), so flush the microtasks that issue the first request.
    await Promise.resolve();
    await Promise.resolve();
  });

  // The hook asks for the count first, then for every transcript in one page. They are told
  // apart by order, not by `limit`: with a single transcript both requests carry `limit: 1`.
  const requests = () => pending.filter(entry => entry.command === 'api_get_meeting_transcripts');

  const countRequest = requests()[0];
  expect(countRequest).toBeDefined();

  await act(async () => {
    countRequest.resolve({ transcripts: [], total_count: transcripts.length, has_more: false });
  });

  const allRequest = requests()[1];
  expect(allRequest).toBeDefined();

  await act(async () => {
    allRequest.resolve({ transcripts, total_count: transcripts.length, has_more: false });
  });

  // Names given to speakers in this meeting are read last.
  const namesRequest = pending.find(entry => entry.command === 'api_get_speaker_names');
  expect(namesRequest).toBeDefined();
  await act(async () => {
    namesRequest!.resolve(speakerNames);
  });

  return written.join('');
}

describe('copying a transcript', () => {
  test('a speaker label is included in the copied text', async () => {
    const text = await copyWith([
      { id: 'a', text: '你好', timestamp: '00:00', audio_start_time: 0, speaker: 'speaker_1' },
    ]);

    expect(text).toContain('说话人 1');
    expect(text).toContain('你好');
  });

  test('the speaker appears on each line rather than once per run', async () => {
    // Pasted text is quoted and reordered, so a label that depended on the line above it
    // would be lost.
    const text = await copyWith([
      { id: 'a', text: '第一句', timestamp: '00:00', audio_start_time: 0, speaker: 'speaker_1' },
      { id: 'b', text: '第二句', timestamp: '00:05', audio_start_time: 5, speaker: 'speaker_1' },
    ]);

    expect(text.split('说话人 1').length - 1).toBe(2);
  });

  test('a transcript without speakers is copied exactly as before', async () => {
    const text = await copyWith([
      { id: 'a', text: 'no speaker here', timestamp: '00:00', audio_start_time: 0 },
    ]);

    expect(text).toContain('no speaker here');
    expect(text).not.toContain('说话人');
  });

  test('different speakers are labelled differently', async () => {
    const text = await copyWith([
      { id: 'a', text: '甲', timestamp: '00:00', audio_start_time: 0, speaker: 'speaker_1' },
      { id: 'b', text: '乙', timestamp: '00:05', audio_start_time: 5, speaker: 'speaker_2' },
    ]);

    expect(text).toContain('说话人 1');
    expect(text).toContain('说话人 2');
  });

  test('a speaker the user named is copied under that name', async () => {
    const text = await copyWith([
      { id: 'a', text: '甲', timestamp: '00:00', audio_start_time: 0, speaker: 'speaker_1' },
      { id: 'b', text: '乙', timestamp: '00:05', audio_start_time: 5, speaker: 'speaker_2' },
    ], { speaker_1: '张总' });

    expect(text).toContain('张总 [00:00] 甲');
    expect(text).toContain('说话人 2 [00:05] 乙');
  });
});
