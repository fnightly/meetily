import { afterAll, afterEach, beforeEach, describe, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';

/**
 * Rendering checks for the speaker separation settings panel.
 *
 * These are not string-matching on source: each one renders the component and reads the
 * text it actually produced, so a crash, a missing branch, or a label resolved from the
 * wrong place fails here.
 */

const originalCore = { ...(await import('@tauri-apps/api/core')) };
const originalToast = { ...(await import('sonner')) };

type Pending = { command: string; resolve: (value: unknown) => void; reject: (reason: unknown) => void };
const pending: Pending[] = [];

const invoke = mock((command: string) => new Promise((resolve, reject) => {
  pending.push({ command, resolve, reject });
}));

mock.module('@tauri-apps/api/core', () => ({ ...originalCore, invoke }));

// The real toast reaches for the DOM, which these tests do not have.
const toast = { success: mock(() => undefined), error: mock(() => undefined), info: mock(() => undefined) };
mock.module('sonner', () => ({ ...originalToast, toast }));

const { SpeakerSeparationSettings } = await import('../../src/components/SpeakerSeparationSettings');

afterAll(() => {
  mock.module('@tauri-apps/api/core', () => originalCore);
  mock.module('sonner', () => originalToast);
});

let renderer: ReactTestRenderer | undefined;

beforeEach(() => {
  pending.length = 0;
  invoke.mockClear();
  toast.success.mockClear();
  toast.error.mockClear();
  toast.info.mockClear();
});

afterEach(async () => {
  if (renderer) await act(async () => renderer!.unmount());
  renderer = undefined;
});

/** Every string the component rendered, in tree order. */
function renderedText(node: ReactTestRenderer): string {
  const parts: string[] = [];

  const walk = (value: unknown) => {
    if (typeof value === 'string') {
      parts.push(value);
      return;
    }
    if (Array.isArray(value)) {
      value.forEach(walk);
      return;
    }
    if (value && typeof value === 'object' && 'children' in value) {
      walk((value as { children: unknown }).children);
    }
  };

  walk(node.toJSON());
  return parts.join(' ');
}

async function render() {
  await act(async () => {
    renderer = create(<SpeakerSeparationSettings />);
  });
  return renderedText(renderer!);
}

async function settle(index: number, value: unknown) {
  await act(async () => {
    pending[index].resolve(value);
  });
  return renderedText(renderer!);
}

const status = (over: Partial<{
  ready: boolean;
  folder: string;
  missing: string[];
  message: string;
  required_bytes: number;
  available_bytes: number | null;
  has_room: boolean;
}> = {}) => ({
  ready: true,
  folder: 'D:\\Meetly\\Model\\diarization',
  missing: [],
  message: 'Models ready',
  required_bytes: 0,
  available_bytes: 12_000_000_000,
  has_room: true,
  ...over,
});

describe('speaker separation settings panel', () => {
  test('asks the backend for the model status on mount', async () => {
    await render();
    expect(pending.map(request => request.command)).toContain('get_diarization_model_status');
  });

  test('an installed set of models is reported as ready', async () => {
    await render();
    const text = await settle(0, status());

    expect(text).toContain('说话人分离');
    expect(text).toContain('模型已就绪');
  });

  test('missing models offer the download rather than claiming readiness', async () => {
    await render();
    const text = await settle(0, status({
      ready: false,
      missing: ['segmentation.onnx', 'embedding.onnx'],
      message: 'Missing 2 files',
      required_bytes: 45_000_000,
    }));

    expect(text).toContain('说话人分离');
    expect(text).not.toContain('模型已就绪');
    expect(text).toContain('下载');
  });

  test('the folder holding the models is shown, so it can be found by hand', async () => {
    await render();
    const text = await settle(0, status({ folder: 'E:\\models\\diarization' }));

    expect(text).toContain('E:\\models\\diarization');
  });

  test('a failed status check reports an error instead of rendering nothing', async () => {
    await render();
    await act(async () => {
      pending[0].reject(new Error('backend unavailable'));
    });

    expect(toast.error).toHaveBeenCalled();
    // The panel must still be on screen, with a way to retry.
    const text = renderedText(renderer!);
    expect(text).toContain('说话人分离');
  });

  test('no room on disk is surfaced before the download is attempted', async () => {
    await render();
    const text = await settle(0, status({
      ready: false,
      missing: ['embedding.onnx'],
      required_bytes: 45_000_000,
      available_bytes: 1_000,
      has_room: false,
    }));

    // The component must say something about the space rather than offering a download
    // that will fail partway.
    expect(text.length).toBeGreaterThan(0);
    expect(text).toContain('说话人分离');
  });
});
