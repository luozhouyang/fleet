import { SessionStatus } from '@agent-fleet/protocol-types';
import { describe, expect, it } from 'vitest';
import {
  activityLabel,
  activityTone,
  branchLabel,
  pendingPromptCount,
  usageLabel,
} from './session-header';

describe('usage label', () => {
  it('renders a context percentage and cost', () => {
    expect(usageLabel({ used: 25, size: 100, cost: 0.5 })).toBe('25% context · $0.50');
    expect(usageLabel({ used: 25, size: 100 })).toBe('25% context');
  });

  it('falls back to a raw count and hides empty reports', () => {
    expect(usageLabel({ used: 4000 })).toBe('4000 used');
    expect(usageLabel(null)).toBeNull();
    expect(usageLabel({})).toBeNull();
  });

  it('ignores a zero cost', () => {
    expect(usageLabel({ used: 1, size: 10, cost: 0 })).toBe('10% context');
  });
});

describe('activity', () => {
  it('maps states to tones', () => {
    expect(activityTone({ state: 'running', agent_initiated: false }, undefined)).toBe('running');
    expect(activityTone({ state: 'requires_action', agent_initiated: false }, undefined)).toBe(
      'waiting',
    );
    expect(activityTone({ state: 'idle', agent_initiated: false }, undefined)).toBe('idle');
    expect(activityTone(null, SessionStatus.Running)).toBeNull();
  });

  it('labels background and autonomous work', () => {
    expect(
      activityLabel({ state: 'running', agent_initiated: false, background: ['tests'] }, undefined),
    ).toBe('Background: tests');
    expect(activityLabel({ state: 'running', agent_initiated: true }, undefined)).toBe(
      'Working autonomously',
    );
    expect(activityLabel({ state: 'requires_action', agent_initiated: false }, undefined)).toBe(
      'Needs you',
    );
  });
});

describe('header helpers', () => {
  it('truncates long branch names', () => {
    expect(branchLabel('main')).toBe('main');
    expect(branchLabel('feature/very-long-branch-name-that-keeps-going', 20)).toHaveLength(20);
    expect(branchLabel('   ')).toBeNull();
    expect(branchLabel(undefined)).toBeNull();
  });

  it('sums blocked prompts', () => {
    expect(pendingPromptCount(2, 1)).toBe(3);
    expect(pendingPromptCount(0, 0)).toBe(0);
  });
});
