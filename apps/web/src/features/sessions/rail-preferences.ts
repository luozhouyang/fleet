/**
 * Persisted rail preferences: group mode, collapsed groups and sort toggle.
 *
 * Preferences are per browser, not per account: they describe how this
 * dashboard is laid out, and syncing them across devices would surprise a user
 * who arranged a phone differently from a laptop.
 */

import { useCallback, useEffect, useState } from 'react';
import type { RailGroupMode } from './rail-model';

const STORAGE_KEY = 'fleet.rail.v1';

export interface RailPreferences {
  groupMode: RailGroupMode;
  collapsedGroups: string[];
  /** Whether smart (attention) sorting is on. */
  smartSort: boolean;
}

const DEFAULTS: RailPreferences = {
  groupMode: 'machine',
  collapsedGroups: [],
  smartSort: false,
};

/** Read preferences, tolerating corrupt or absent storage. */
export function readRailPreferences(): RailPreferences {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULTS;
    const value = JSON.parse(raw) as Partial<RailPreferences>;
    const groupMode: RailGroupMode =
      value.groupMode === 'none' ||
      value.groupMode === 'machine' ||
      value.groupMode === 'workspace' ||
      value.groupMode === 'status'
        ? value.groupMode
        : DEFAULTS.groupMode;
    return {
      groupMode,
      collapsedGroups: Array.isArray(value.collapsedGroups)
        ? value.collapsedGroups.filter((key): key is string => typeof key === 'string')
        : [],
      smartSort: value.smartSort === true,
    };
  } catch {
    return DEFAULTS;
  }
}

function writeRailPreferences(preferences: RailPreferences): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(preferences));
  } catch {
    // Preferences are cosmetic; losing them only resets the layout.
  }
}

/** Rail preferences with a stable setter and cross-tab updates. */
export function useRailPreferences(): {
  preferences: RailPreferences;
  setGroupMode: (mode: RailGroupMode) => void;
  toggleGroup: (key: string) => void;
  setSmartSort: (value: boolean) => void;
} {
  const [preferences, setPreferences] = useState<RailPreferences>(readRailPreferences);

  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key === STORAGE_KEY) setPreferences(readRailPreferences());
    };
    window.addEventListener('storage', onStorage);
    return () => window.removeEventListener('storage', onStorage);
  }, []);

  const update = useCallback((next: RailPreferences) => {
    writeRailPreferences(next);
    setPreferences(next);
  }, []);

  const setGroupMode = useCallback(
    (mode: RailGroupMode) => update({ ...readRailPreferences(), groupMode: mode }),
    [update],
  );

  const toggleGroup = useCallback(
    (key: string) => {
      const current = readRailPreferences();
      const collapsed = new Set(current.collapsedGroups);
      if (collapsed.has(key)) collapsed.delete(key);
      else collapsed.add(key);
      update({ ...current, collapsedGroups: [...collapsed] });
    },
    [update],
  );

  const setSmartSort = useCallback(
    (value: boolean) => update({ ...readRailPreferences(), smartSort: value }),
    [update],
  );

  return { preferences, setGroupMode, toggleGroup, setSmartSort };
}
