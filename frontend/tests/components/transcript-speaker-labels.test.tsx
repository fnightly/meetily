import { afterEach, describe, expect, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { VirtualizedTranscriptView } from '../../src/components/VirtualizedTranscriptView';
import { TooltipProvider } from '../../src/components/ui/tooltip';
import type { TranscriptSegmentData } from '../../src/types';

/**
 * Rendering checks for the speaker labels in the transcript list.
 *
 * The list is virtualized, so the speaker name is shown only where it changes — these
 * render the component and read the text it produced, which is the only way to check that
 * rule short of looking at the window.
 */

let renderer: ReactTestRenderer | undefined;

afterEach(async () => {
  if (renderer) await act(async () => renderer!.unmount());
  renderer = undefined;
});

function renderedText(): string {
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

  walk(renderer!.toJSON());
  return parts.join(' ');
}

async function show(segments: TranscriptSegmentData[]) {
  await act(async () => {
    renderer = create(
      <TooltipProvider>
        <VirtualizedTranscriptView
          segments={segments}
          showConfidence={false}
          disableAutoScroll
        />
      </TooltipProvider>
    );
  });
  return renderedText();
}

const segment = (
  id: string,
  seconds: number,
  text: string,
  speaker?: string | null
): TranscriptSegmentData => ({
  id,
  timestamp: seconds,
  text,
  speaker,
});

describe('speaker labels in the transcript list', () => {
  test('a labelled segment shows its speaker', async () => {
    const text = await show([segment('a', 0, '你好', 'speaker_1')]);
    expect(text).toContain('说话人 1');
  });

  test('the label is shown once for a run of one speaker, not on every line', async () => {
    // Consecutive turns by the same person must read as one block; repeating the name is
    // what the grouping exists to avoid.
    const text = await show([
      segment('a', 0, '第一句', 'speaker_1'),
      segment('b', 5, '第二句', 'speaker_1'),
      segment('c', 10, '第三句', 'speaker_1'),
    ]);

    expect(text.split('说话人 1').length - 1).toBe(1);
    expect(text).toContain('第一句');
    expect(text).toContain('第三句');
  });

  test('the label is shown again when the speaker changes', async () => {
    const text = await show([
      segment('a', 0, '甲', 'speaker_1'),
      segment('b', 5, '乙', 'speaker_2'),
      segment('c', 10, '丙', 'speaker_1'),
    ]);

    expect(text).toContain('说话人 1');
    expect(text).toContain('说话人 2');
    // Speaker 1 returns after speaker 2, so their name appears twice in total.
    expect(text.split('说话人 1').length - 1).toBe(2);
  });

  test('a meeting that was never separated shows no speaker labels', async () => {
    // The common case: separation has not been run, and a default label would attribute
    // speech to someone who was never identified.
    const text = await show([
      segment('a', 0, '没有标注', null),
      segment('b', 5, '也没有'),
    ]);

    expect(text).not.toContain('说话人');
    expect(text).toContain('没有标注');
  });

  test('speaker numbering follows the stored label', async () => {
    const text = await show([
      segment('a', 0, '甲', 'speaker_3'),
      segment('b', 5, '乙', 'speaker_12'),
    ]);

    expect(text).toContain('说话人 3');
    expect(text).toContain('说话人 12');
  });

  test('an unparseable label is shown as-is rather than dropped', async () => {
    // A label the interface cannot parse is still information; hiding it would leave a
    // turns of speech looking unattributed.
    const text = await show([segment('a', 0, '甲', 'Unknown')]);
    expect(text).toContain('Unknown');
  });

  test('the transcript text is shown whatever the speaker state', async () => {
    const text = await show([
      segment('a', 0, '有说话人的内容', 'speaker_1'),
      segment('b', 5, '没有说话人的内容', null),
    ]);

    expect(text).toContain('有说话人的内容');
    expect(text).toContain('没有说话人的内容');
  });
});
