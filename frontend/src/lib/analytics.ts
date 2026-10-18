/**
 * Usage analytics has been removed from the product.
 *
 * The call sites that used to report button clicks, meeting metrics and backend
 * events are still present in the UI code. This compatibility layer keeps them
 * compiling while guaranteeing that no analytics data is written or sent: every
 * method is intentionally inert. New code should not call these methods.
 */

export interface AnalyticsProperties {
  [key: string]: string;
}

export interface DeviceInfo {
  platform: string;
  os_version: string;
  architecture: string;
}

export interface UserSession {
  session_id: string;
  user_id: string;
  start_time: string;
  last_heartbeat: string;
  is_active: boolean;
}

export default class Analytics {
  static async init(..._args: unknown[]): Promise<void> {}
  static async disable(..._args: unknown[]): Promise<void> {}
  static async isEnabled(..._args: unknown[]): Promise<boolean> { return false; }
  static async track(..._args: unknown[]): Promise<void> {}
  static async identify(..._args: unknown[]): Promise<void> {}
  static async startSession(..._args: unknown[]): Promise<string | null> { return null; }
  static async endSession(..._args: unknown[]): Promise<void> {}
  static async trackDailyActiveUser(..._args: unknown[]): Promise<void> {}
  static async trackUserFirstLaunch(..._args: unknown[]): Promise<void> {}
  static async isSessionActive(..._args: unknown[]): Promise<boolean> { return false; }
  static async getPersistentUserId(..._args: unknown[]): Promise<string> { return ''; }
  static async checkAndTrackFirstLaunch(..._args: unknown[]): Promise<void> {}
  static async checkAndTrackDailyUsage(..._args: unknown[]): Promise<void> {}
  static getCurrentUserId(..._args: unknown[]): string | null { return null; }
  static async getPlatform(..._args: unknown[]): Promise<string> { return 'unknown'; }
  static async getOSVersion(..._args: unknown[]): Promise<string> { return 'unknown'; }
  static async getDeviceInfo(..._args: unknown[]): Promise<DeviceInfo> {
    return { platform: 'unknown', os_version: 'unknown', architecture: 'unknown' };
  }
  static async calculateDaysSince(..._args: unknown[]): Promise<number | null> { return null; }
  static async updateMeetingCount(..._args: unknown[]): Promise<void> {}
  static async getMeetingsCountToday(..._args: unknown[]): Promise<number> { return 0; }
  static async hasUsedFeatureBefore(..._args: unknown[]): Promise<boolean> { return false; }
  static async markFeatureUsed(..._args: unknown[]): Promise<void> {}
  static async trackSessionStarted(..._args: unknown[]): Promise<void> {}
  static async trackSessionEnded(..._args: unknown[]): Promise<void> {}
  static async trackMeetingCompleted(..._args: unknown[]): Promise<void> {}
  static async trackFeatureUsedEnhanced(..._args: unknown[]): Promise<void> {}
  static async trackCopy(..._args: unknown[]): Promise<void> {}
  static async trackFeatureUsed(..._args: unknown[]): Promise<void> {}
  static async trackPageView(..._args: unknown[]): Promise<void> {}
  static async trackButtonClick(..._args: unknown[]): Promise<void> {}
  static async trackError(..._args: unknown[]): Promise<void> {}
  static async trackAppStarted(..._args: unknown[]): Promise<void> {}
  static async cleanup(..._args: unknown[]): Promise<void> {}
  static reset(..._args: unknown[]): void {}
  static async waitForInitialization(..._args: unknown[]): Promise<boolean> { return false; }
  static async trackBackendConnection(..._args: unknown[]): Promise<void> {}
  static async trackSettingsChanged(..._args: unknown[]): Promise<void> {}
  static async trackMeetingStarted(..._args: unknown[]): Promise<void> {}
  static async trackRecordingStarted(..._args: unknown[]): Promise<void> {}
  static async trackRecordingStopped(..._args: unknown[]): Promise<void> {}
  static async trackMeetingDeleted(..._args: unknown[]): Promise<void> {}
  static async trackModelChanged(..._args: unknown[]): Promise<void> {}
  static async trackSummaryGenerationStarted(..._args: unknown[]): Promise<void> {}
  static async trackSummaryGenerationCompleted(..._args: unknown[]): Promise<void> {}
  static async trackSummaryRegenerated(..._args: unknown[]): Promise<void> {}
  static async trackCustomPromptUsed(..._args: unknown[]): Promise<void> {}
  static async trackTranscriptionError(..._args: unknown[]): Promise<void> {}
  static async trackTranscriptionSuccess(..._args: unknown[]): Promise<void> {}
}
