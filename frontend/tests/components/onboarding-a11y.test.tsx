import { afterEach, describe, expect, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { ProgressIndicator } from '../../src/components/onboarding/shared/ProgressIndicator';
import { PermissionRow } from '../../src/components/onboarding/shared/PermissionRow';

/**
 * Onboarding is the first surface a user sees and renders outside a provider in
 * these checks, so the i18n fallback locale (zh-CN) is what gets asserted. That
 * also catches a missing onboarding.* key: translate() falls back to the key
 * itself, not to empty text.
 */

let renderer: ReactTestRenderer | undefined;

afterEach(async () => {
  if (renderer) await act(async () => renderer!.unmount());
  renderer = undefined;
});

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

describe('onboarding accessibility and localization', () => {
  test('step indicators have names and mark the current step', async () => {
    await act(async () => {
      renderer = create(<ProgressIndicator current={2} total={4} />);
    });
    const root = renderer!.root;
    const buttons = root.findAllByType('button');
    expect(buttons).toHaveLength(4);
    expect(buttons[0].props['aria-label']).toBe('第 1 步，共 4 步');
    expect(buttons[1].props['aria-current']).toBe('step');
    expect(buttons[2].props['aria-current']).toBeUndefined();
  });

  test('permission status and action are translated', async () => {
    await act(async () => {
      renderer = create(
        <PermissionRow
          icon={<span />}
          title="麦克风"
          description="用于在会议中录制你的声音"
          status="denied"
          onAction={() => {}}
        />,
      );
    });
    const text = renderedText(renderer!);
    expect(text).toContain('未授权——请在系统设置中开启');
    expect(text).toContain('打开设置');
    expect(text).not.toContain('onboarding.permissions');
  });
});
