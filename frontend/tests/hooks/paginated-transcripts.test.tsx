import { afterAll, afterEach, beforeEach, describe, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import type { MeetingMetadata, PaginatedTranscriptsResponse } from '../../src/types';

const originalCore = { ...await import('@tauri-apps/api/core') };
type Request = {
  command: string;
  args: Record<string, unknown>;
  resolve: (value: unknown) => void;
  reject: (reason: unknown) => void;
};
const requests: Request[] = [];
const invoke = mock((command: string, args: Record<string, unknown>) => new Promise((resolve, reject) => {
  requests.push({ command, args, resolve, reject });
}));
mock.module('@tauri-apps/api/core', () => ({ ...originalCore, invoke }));
const { usePaginatedTranscripts } = await import('../../src/hooks/usePaginatedTranscripts');
afterAll(() => mock.module('@tauri-apps/api/core', () => originalCore));

let state: ReturnType<typeof usePaginatedTranscripts>;
let renderer: ReactTestRenderer | undefined;
function View({ meetingId }: { meetingId: string | null }) {
  state = usePaginatedTranscripts({ meetingId });
  return <output>{state.metadata?.id}:{state.transcripts.map(t => t.text).join(',')}</output>;
}
beforeEach(() => {
  requests.length = 0;
  invoke.mockClear();
  mock.module('@tauri-apps/api/core', () => ({ ...originalCore, invoke }));
});
afterEach(async () => {
  if (renderer) await act(async () => renderer!.unmount());
  renderer = undefined;
});
async function show(meetingId: string | null) {
  await act(async () => {
    if (renderer) renderer.update(<View meetingId={meetingId} />);
    else renderer = create(<View meetingId={meetingId} />);
  });
}
function request(command: 'metadata' | 'transcripts', meetingId: string, index = 0) {
  const result = requests.filter(r => r.command === `api_get_meeting_${command}` && r.args.meetingId === meetingId)[index];
  expect(result).toBeDefined();
  return result;
}
const metadata = (id: string): MeetingMetadata => ({ id, title: id, created_at: '2026-09-07', updated_at: '2026-09-07' });
const page = (text: string, hasMore = false): PaginatedTranscriptsResponse => ({
  transcripts: [{ id: text, text, timestamp: '00:00', audio_start_time: 0 }],
  total_count: hasMore ? 2 : 1, has_more: hasMore,
});
async function resolve(req: Request, value: unknown) {
  await act(async () => { req.resolve(value); });
}
async function load(meetingId: string, hasMore = false) {
  await show(meetingId);
  await resolve(request('metadata', meetingId), metadata(meetingId));
  await resolve(request('transcripts', meetingId), page(meetingId, hasMore));
}

describe('paginated transcript request ownership', () => {
  test('ignores late A metadata after B finishes and does not start A transcripts', async () => {
    await show('A');
    const stale = request('metadata', 'A');
    await load('B');
    await resolve(stale, metadata('A'));
    expect(state.metadata?.id).toBe('B');
    expect(state.transcripts[0].text).toBe('B');
    expect(state.isLoading).toBe(false);
    expect(requests.filter(r => r.command === 'api_get_meeting_transcripts' && r.args.meetingId === 'A')).toHaveLength(0);
  });

  test('late A transcripts cannot replace B or finish its loading state', async () => {
    await show('A');
    await resolve(request('metadata', 'A'), metadata('A'));
    const stale = request('transcripts', 'A');
    await show('B');
    await resolve(stale, page('A'));
    expect(state.isLoading).toBe(true);
    expect(state.transcripts).toEqual([]);
    await resolve(request('metadata', 'B'), metadata('B'));
    await resolve(request('transcripts', 'B'), page('B'));
    expect(state.metadata?.id).toBe('B');
    expect(state.transcripts[0].text).toBe('B');
  });

  test('returning A to B to A does not revive the first A request', async () => {
    await show('A');
    const stale = request('metadata', 'A');
    await show('B');
    await show('A');
    await resolve(request('metadata', 'A', 1), { ...metadata('A'), title: 'Current A' });
    await resolve(request('transcripts', 'A'), page('Current A'));
    await resolve(stale, { ...metadata('A'), title: 'Old A' });
    expect(state.metadata?.title).toBe('Current A');
    expect(state.transcripts[0].text).toBe('Current A');
    expect(state.isLoading).toBe(false);
    expect(requests.filter(r => r.command === 'api_get_meeting_transcripts' && r.args.meetingId === 'A')).toHaveLength(1);
  });

  test('a stale failure cannot add an error or clear current loading', async () => {
    await show('A');
    const stale = request('metadata', 'A');
    await show('B');
    await act(async () => { stale.reject(new Error('A unavailable')); });
    expect(state.error).toBeNull();
    expect(state.isLoading).toBe(true);
  });

  test('newest refetch wins over an earlier request for the same meeting', async () => {
    await load('A');
    await act(async () => { void state.refetch(); });
    await resolve(request('metadata', 'A', 1), metadata('A'));
    const stale = request('transcripts', 'A', 1);
    await act(async () => { void state.refetch(); });
    await resolve(request('metadata', 'A', 2), metadata('A'));
    await resolve(request('transcripts', 'A', 2), page('newest'));
    await resolve(stale, page('stale'));
    expect(state.transcripts[0].text).toBe('newest');
    expect(state.isLoading).toBe(false);
  });

  test('old loadMore cannot append data or clear the next meeting loadMore lock', async () => {
    await load('A', true);
    await act(async () => { void state.loadMore(); });
    const stale = request('transcripts', 'A', 1);
    await load('B', true);
    await act(async () => { void state.loadMore(); });
    const current = request('transcripts', 'B', 1);
    await resolve(stale, page('old extra'));
    expect(state.isLoadingMore).toBe(true);
    expect(state.transcripts.map(t => t.text)).toEqual(['B']);
    await resolve(current, page('B extra'));
    expect(state.isLoadingMore).toBe(false);
    expect(state.transcripts.map(t => t.text)).toEqual(['B', 'B extra']);
  });

  test('reset invalidates pending reads and null to same meeting reloads', async () => {
    await show('A');
    const stale = request('metadata', 'A');
    await act(async () => { state.reset(); });
    await resolve(stale, metadata('A'));
    expect(state.metadata).toBeNull();
    await show(null);
    await show('A');
    await resolve(request('metadata', 'A', 1), metadata('A'));
    await resolve(request('transcripts', 'A'), page('fresh'));
    expect(state.transcripts[0].text).toBe('fresh');
  });

  test('unmount prevents a pending metadata read from starting transcript IPC', async () => {
    await show('A');
    const stale = request('metadata', 'A');
    await act(async () => { renderer!.unmount(); });
    renderer = undefined;
    await resolve(stale, metadata('A'));
    expect(requests).toHaveLength(1);
  });

  test('a speaker label reaches the segments the list renders', async () => {
    // The meeting view paginates, so it renders these segments rather than mapping the
    // transcripts itself. A label that stops at this conversion is stored correctly and
    // then never displayed.
    await show('A');
    await resolve(request('metadata', 'A'), metadata('A'));
    await resolve(request('transcripts', 'A'), {
      transcripts: [
        { id: 'a', text: 'first', timestamp: '00:00', audio_start_time: 0, speaker: 'speaker_1' },
        { id: 'b', text: 'second', timestamp: '00:05', audio_start_time: 5, speaker: 'speaker_2' },
      ],
      total_count: 2,
      has_more: false,
    });

    expect(state.segments?.map(segment => segment.speaker)).toEqual(['speaker_1', 'speaker_2']);
  });

  test('transcripts without a speaker leave the field absent rather than guessed', async () => {
    // Separation has not been run for most meetings, and a default would label everyone
    // as the first speaker.
    await show('A');
    await resolve(request('metadata', 'A'), metadata('A'));
    await resolve(request('transcripts', 'A'), page('unseparated'));

    expect(state.segments?.[0].speaker).toBeUndefined();
  });

  test('a speaker label survives loading more segments', async () => {
    // The second page is appended to the first, so a field dropped on the append path
    // would make labels disappear as the user scrolls.
    await load('A', true);
    await act(async () => { void state.loadMore(); });
    await resolve(request('transcripts', 'A', 1), {
      transcripts: [{ id: 'more', text: 'more', timestamp: '00:10', audio_start_time: 10, speaker: 'speaker_3' }],
      total_count: 2,
      has_more: false,
    });

    expect(state.segments?.map(segment => segment.speaker)).toEqual([undefined, 'speaker_3']);
  });
});
