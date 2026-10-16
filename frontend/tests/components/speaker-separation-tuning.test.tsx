import { afterAll, afterEach, beforeEach, describe, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';

/**
 * Rendering checks for the separation tuning panel.
 *
 * The panel is collapsed until asked for, so these drive the toggle the way a person would
 * and read the text and the request that came out.
 */

const originalCore = { ...(await import('@tauri-apps/api/core')) };
const originalToast = { ...(await import('sonner')) };

type Pending = { command: string; args: Record<string, unknown>; resolve: (value: unknown) => void; reject: (reason: unknown) => void };
const pending: Pending[] = [];

const invoke = mock((command: string, args: Record<string, unknown> = {}) => new Promise((resolve, reject) => {
  pending.push({ command, args, resolve, reject });
}));

mock.module('@tauri-apps/api/core', () => ({ ...originalCore, invoke }));

const toast = { success: mock(() => undefined), error: mock(() => undefined), info: mock(() => undefined) };
mock.module('sonner', () => ({ ...originalToast, toast }));

const { SpeakerSeparationTuning } = await import('../../src/components/SpeakerSeparationTuning');

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
});

afterEach(async () => {
  if (renderer) await act(async () => renderer!.unmount());
  renderer = undefined;
});

const view = (over: Partial<{ threshold: number; max_speakers: number }> = {}, isDefault = false) => ({
  preferences: { threshold: 0.75, max_speakers: 12, ...over },
  bounds: {
    min_threshold: 0.4,
    max_threshold: 0.95,
    min_speakers: 2,
    max_speakers: 32,
    calibrated_threshold: 0.75,
  },
  is_default: isDefault,
});

function text(): string {
  const parts: string[] = [];
  const walk = (value: unknown) => {
    if (typeof value === 'string') { parts.push(value); return; }
    if (typeof value === 'number') { parts.push(String(value)); return; }
    if (Array.isArray(value)) { value.forEach(walk); return; }
    if (value && typeof value === 'object' && 'children' in value) {
      walk((value as { children: unknown }).children);
    }
  };
  walk(renderer!.toJSON());
  return parts.join(' ');
}

/** All text rendered beneath a node, so a button can be found by its label. */
function subtreeText(node: unknown): string {
  const parts: string[] = [];
  const walk = (value: unknown) => {
    if (typeof value === 'string' || typeof value === 'number') { parts.push(String(value)); return; }
    if (Array.isArray(value)) { value.forEach(walk); return; }
    if (value && typeof value === 'object' && 'children' in value) {
      walk((value as { children: unknown }).children);
    }
  };
  walk(node);
  return parts.join('');
}

/** Every rendered host node, with the text beneath it. */
function nodes(): Array<{ type: unknown; props: Record<string, unknown>; text: string }> {
  const found: Array<{ type: unknown; props: Record<string, unknown>; text: string }> = [];
  const walk = (node: unknown) => {
    if (!node || typeof node !== 'object') return;
    const element = node as { type?: unknown; props?: Record<string, unknown>; children?: unknown };
    if (element.type) {
      found.push({ type: element.type, props: element.props ?? {}, text: subtreeText(element.children) });
    }
    if (Array.isArray(element.children)) element.children.forEach(walk);
    else if (element.children) walk(element.children);
  };
  walk(renderer!.toJSON());
  return found;
}

/** The nodes a person could click. */
function clickable() {
  return nodes().filter(node => typeof node.props.onClick === 'function');
}

/** The button whose label contains `label`. */
function button(label: string) {
  const found = clickable().filter(node => node.type === 'button' && node.text.includes(label));
  expect(found.length).toBeGreaterThan(0);
  return found[0];
}

/** The number inputs, in the order they are rendered. */
function inputs(): Array<{ props: Record<string, unknown> }> {
  return nodes().filter(node => node.type === 'input').map(node => ({ props: node.props }));
}

async function render() {
  await act(async () => {
    renderer = create(<SpeakerSeparationTuning />);
  });
}

async function load(value: unknown) {
  await act(async () => { pending[0].resolve(value); });
}

async function openPanel() {
  const header = button('高级设置');
  await act(async () => { (header.props.onClick as () => void)(); });
}

describe('speaker separation tuning', () => {
  test('reads the stored settings on mount', async () => {
    await render();
    expect(pending[0]?.command).toBe('get_diarization_preferences');
  });

  test('the controls are hidden until asked for', async () => {
    await render();
    await load(view());

    // Collapsed, the panel states what it is for but shows no fields.
    expect(text()).toContain('高级设置');
    expect(inputs()).toHaveLength(0);
  });

  test('opening the panel reveals both settings with their limits', async () => {
    await render();
    await load(view());
    await openPanel();

    const rendered = text();
    expect(rendered).toContain('聚类阈值');
    expect(rendered).toContain('说话人上限');
    // The bands must be visible, so a refused value is explicable.
    expect(rendered).toContain('0.4');
    expect(rendered).toContain('0.95');
  });

  test('the calibrated value is stated so the user knows what to return to', async () => {
    await render();
    await load(view());
    await openPanel();

    expect(text()).toContain('0.75');
  });

  test('the stored values are shown in the fields', async () => {
    await render();
    await load(view({ threshold: 0.62, max_speakers: 6 }, false));
    await openPanel();

    const values = inputs().map(input => input.props.value);
    expect(values).toEqual(['0.62', '6']);
  });

  test('non-default values are flagged', async () => {
    await render();
    await load(view({ threshold: 0.62 }, false));
    await openPanel();

    expect(text()).toContain('已不是校准默认值');
  });

  test('the defaults are not flagged as changed', async () => {
    await render();
    await load(view({}, true));
    await openPanel();

    expect(text()).not.toContain('已不是校准默认值');
  });

  test('saving sends the numbers the user typed', async () => {
    await render();
    await load(view());
    await openPanel();

    const fields = inputs();
    await act(async () => {
      (fields[0].props.onChange as (e: { target: { value: string } }) => void)({ target: { value: '0.62' } });
      (fields[1].props.onChange as (e: { target: { value: string } }) => void)({ target: { value: '6' } });
    });

    const saveButton = button('保存');
    await act(async () => { (saveButton.props.onClick as () => void)(); });

    const request = pending.find(entry => entry.command === 'set_diarization_preferences');
    expect(request).toBeDefined();
    expect(request!.args.preferences).toEqual({ threshold: 0.62, max_speakers: 6 });
  });

  test('what the backend stored is shown back, not what was typed', async () => {
    // A clamped value must be visible, or the field would keep displaying a number that was
    // never accepted.
    await render();
    await load(view());
    await openPanel();

    const fields = inputs();
    await act(async () => {
      (fields[0].props.onChange as (e: { target: { value: string } }) => void)({ target: { value: '0.01' } });
    });

    const saveButton = button('保存');
    await act(async () => { (saveButton.props.onClick as () => void)(); });

    const request = pending.find(entry => entry.command === 'set_diarization_preferences');
    await act(async () => { request!.resolve(view({ threshold: 0.4 }, false)); });

    expect(inputs()[0].props.value).toBe('0.4');
  });

  test('an empty field is refused without calling the backend', async () => {
    await render();
    await load(view());
    await openPanel();

    const fields = inputs();
    await act(async () => {
      (fields[0].props.onChange as (e: { target: { value: string } }) => void)({ target: { value: '' } });
    });

    const saveButton = button('保存');
    await act(async () => { (saveButton.props.onClick as () => void)(); });

    expect(pending.some(entry => entry.command === 'set_diarization_preferences')).toBe(false);
    expect(toast.error).toHaveBeenCalled();
  });

  test('restoring the defaults calls the reset command', async () => {
    await render();
    await load(view({ threshold: 0.62 }, false));
    await openPanel();

    const resetButton = button('恢复默认');
    await act(async () => { (resetButton.props.onClick as () => void)(); });

    expect(pending.some(entry => entry.command === 'reset_diarization_preferences')).toBe(true);
  });

  test('a failed load reports an error rather than rendering nothing', async () => {
    await render();
    await act(async () => { pending[0].reject(new Error('no store')); });
    // Let the rejection's `finally` clear the loading flag before reading the output.
    await act(async () => { await Promise.resolve(); });

    expect(toast.error).toHaveBeenCalled();
    // The section must stay on screen with a way to try again, rather than vanishing.
    expect(text()).toContain('高级设置');
    expect(text()).toContain('点击重试');

    // Clicking it asks again.
    await act(async () => { (button('高级设置').props.onClick as () => void)(); });
    expect(pending.filter(entry => entry.command === 'get_diarization_preferences')).toHaveLength(2);
  });
});
