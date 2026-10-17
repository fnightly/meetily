/**
 * Minimal internationalisation.
 *
 * The app is used mainly in Chinese, but every user-facing string was English;
 * the Chinese support lived entirely in the backend prompts, so the promise and
 * what the user actually saw did not match.
 *
 * Deliberately hand-rolled rather than pulling in `next-intl` or `i18next`:
 * this is a statically exported Tauri app with no server rendering, the bundle
 * is a stated concern of the project, and the whole requirement is a lookup
 * table plus a persisted choice. Two dependencies are not needed for that.
 *
 * English is the complete locale and the fallback; Chinese is the default for a
 * first run so the app opens in the language most of its users read.
 */

export type Locale = 'en' | 'zh-CN';

export const LOCALES: Locale[] = ['zh-CN', 'en'];

/** Where the choice is persisted. */
export const LOCALE_STORAGE_KEY = 'meetily.locale';

/** The locale used when the user has not chosen one. */
export const DEFAULT_LOCALE: Locale = 'zh-CN';

/** Shown in the settings language selector. */
export const LOCALE_LABELS: Record<Locale, string> = {
  en: 'English',
  'zh-CN': '简体中文',
};

/**
 * String table.
 *
 * Keys are flat and dot-namespaced by surface (`settings.tabs.general`) so a
 * missing translation is obvious at a glance and easy to grep for.
 *
 * Chinese values are written as the interface language, not translated
 * word-for-word from English: "Recordings" as a settings tab is 「录音」, while
 * "Records" as a verb is 「录制」.
 */
const EN: Record<string, string> = {
  'common.cancel': 'Cancel',
  'common.delete': 'Delete',
  'common.rename': 'Rename',
  'common.refresh': 'Refresh',
  'common.reload': 'Reload',
  'common.add': 'Add',
  'common.remove': 'Remove',
  'common.back': 'Back',
  'common.settings': 'Settings',
  'common.loading': 'Loading…',
  'common.unknownDate': 'Unknown date',
  'common.retry': 'Retry',

  'settings.title': 'Settings',
  'settings.tabs.general': 'General',
  'settings.tabs.recording': 'Recordings',
  'settings.tabs.library': 'Library',
  'settings.tabs.dictionary': 'Dictionary',
  'settings.tabs.transcription': 'Transcription',
  'settings.tabs.summary': 'Summary',
  'settings.tabs.beta': 'Beta',
  'settings.language': 'Interface Language',
  'settings.language.description':
    'The language used for buttons, labels and messages. This is separate from the language you transcribe and summarise in.',

  'dictionary.title': 'Custom Dictionary',
  'dictionary.description':
    'Names, product names and jargon the speech model should spell exactly. These are passed to transcription cleanup and to summary generation, so they survive into your notes.',
  'dictionary.correctSpelling': 'Correct spelling',
  'dictionary.misheardAs': 'Often misheard as',
  'dictionary.optional': '(optional)',
  'dictionary.termPlaceholder': 'e.g. a colleague’s name',
  'dictionary.misheardPlaceholder': 'e.g. what it usually comes out as',
  'dictionary.addHint':
    'Adding the misheard form makes the correction much stronger than a bare term list.',
  'dictionary.empty': 'No terms yet. Add the names that keep coming out wrong.',
  'dictionary.count': '{count} of {max} terms used. Edits are saved when you leave a field.',
  'dictionary.limitReached': 'The dictionary is limited to {max} entries',
  'dictionary.loadFailed': 'Could not load your dictionary',
  'dictionary.saveFailed': 'Could not save your dictionary',
  'dictionary.removeTitle': 'Remove this term',

  'nav.home': 'Home',
  'nav.library': 'Meetings & Recordings',
  'nav.settings': 'Settings',
  'nav.about': 'About',
  'nav.searchPlaceholder': 'Search meeting content…',
  'nav.importAudio': 'Import Audio',
  'nav.startRecording': 'Start Recording',
  'nav.recordingInProgress': 'Recording in progress…',
  'nav.searching': 'Searching…',
  'nav.emptyLibrary': 'No meetings or recordings yet. They appear here as you make them.',
  'nav.noMatches': 'Nothing matches “{query}”',
  'nav.openMeeting': 'Open meeting details for {title}',
  'nav.openRecording': 'Open {title} in the recordings library',
  'nav.openNotes': 'Open notes for {title}',
  'nav.renameMeeting': 'Rename {title}',
  'nav.deleteMeeting': 'Delete {title}',
  'nav.match': 'Match:',
  'common.save': 'Save',

  'recordingsPage.title': 'Recordings',
  'recordingsPage.description':
    'Every recording saved on disk. Transcribe, summarise, rename, move or delete each one.',

  'library.title': 'Recording Library',
  'library.description':
    'Every recording saved on disk, newest first. Includes recordings that have no meeting entry, so nothing is hidden.',
  'library.loading': 'Reading the recordings folder…',
  'library.folder': 'Folder',
  'library.summary': '{count} recording(s) · {size} of audio',
  'library.folderMissing':
    'This folder does not exist yet. It is created when you make your first recording.',
  'library.empty': 'No recordings found in this folder.',
  'library.loadFailed': 'Could not read the recordings folder',
  'library.openFolder': "Open this recording's folder",
  'library.renameTitle': 'Rename this recording',
  'library.moveTitle': 'Move this recording elsewhere',
  'library.transcribe': 'Transcribe',
  'library.transcribeTitle': 'Transcribe this recording',
  'library.summarise': 'Summarise',
  'library.resummarise': 'Re-summarise',
  'library.summariseTitle': 'Summarise the transcript',
  'library.removeSummary': 'Remove summary',
  'library.removeSummaryTitle': 'Remove the summary, keeping the transcript',
  'library.deleteTitle': 'Delete this recording and its audio',
  'library.badgeNoAudio': 'No audio',
  'library.badgeIncomplete': 'Incomplete',
  'library.badgeTranscript': 'Transcript',
  'library.badgeSummary': 'Summary',
  'library.noAudioWarning': 'No audio file was found in this folder.',
  'library.footnote':
    'Deleting removes the whole recording folder, including its audio. Moving takes it out of this list because it leaves your recordings folder.',
  'library.renameDialogTitle': 'Rename recording',
  'library.namePlaceholder': 'Recording name',
  'library.deleteDialogTitle': 'Delete this recording?',
  'library.deleteDialogBody':
    '“{name}” will be removed from disk, along with its audio. This cannot be undone.',
  'library.operationFailed': 'The operation failed',
  'library.transcribeStarted': 'Transcription started',
  'library.transcribeStartedDetail': 'You can keep using the app while this runs.',
  'library.transcribeFailed': 'Could not start transcription',

  'recording.limitsTitle': 'Recording Safety Limits',
  'recording.limitsDescription':
    'Guards against recordings that are left running. Both are off by default.',
  'recording.silenceStop': 'Stop after silence',
  'recording.silenceStopHint':
    'Useful for unattended meetings. A quiet ending can be cut off, so the shortest useful value is recommended.',
  'recording.maxLength': 'Maximum recording length',
  'recording.maxLengthHint':
    'Recording stops once this is reached, so a forgotten session cannot fill the disk.',
  'recording.retentionTitle': 'Recording Retention',
  'recording.retentionDescription':
    'Delete saved recordings automatically once they are older than a set number of days. Off by default, so nothing is ever removed unless you choose it.',
  'recording.keepFor': 'Keep recordings for',
  'recording.forever': 'Forever',
  'recording.days': '{days} days',
  'recording.previewPurge': 'Preview what would be deleted',
  'recording.checking': 'Checking…',
  'recording.recordingFormat': 'Recording Format',
  'recording.audioQuality': 'Audio Quality',
  'recording.formatHint': 'Recordings are saved as {file} inside each meeting folder.',
  'recording.qualityHint':
    'Higher quality means larger files. Speech recordings are usually fine at Medium.',
  'recording.losslessNote': '{format} is lossless, so the quality setting does not apply.',
  'recording.lockedWhileRecording':
    'Format and quality are locked while a recording is in progress.',
  'recording.silenceOf': '{duration} of silence',
  'recording.selectFormat': 'Select format',
  'recording.selectQuality': 'Select quality',
  'recording.cleanupTitle': 'Transcript Cleanup',
  'recording.cleanupToggle': 'Clean up the transcript after recording',
  'recording.cleanupDescription':
    'Rewrites the finished transcript with a local model: removes filler words, fixes spoken punctuation, and converts spoken numbers and dates to normal written form. Off by default, because it rewrites your own words. Only local models are used, so the transcript never leaves this machine.',
  'recording.cleanupLocalOnly':
    'Requires a local model (Ollama or the built-in model). If a cloud provider is configured for summaries, cleanup is skipped rather than uploading your transcript.',

  'postRecording.title': 'Recording finished',
  'postRecording.deferredMessage':
    'Transcription was deferred for this recording. Choose what to do with it now.',
  'postRecording.noSpeechMessage':
    'No speech was captured for transcription. Choose what to do with the audio.',
  'postRecording.hasTranscriptMessage':
    'Transcription is already complete. Would you like a summary?',
  'postRecording.transcribe': 'Transcribe',
  'postRecording.transcribeDescription':
    'Turn the recording into text. Runs on the configured speech model.',
  'postRecording.transcribeAndSummarise': 'Transcribe and summarise',
  'postRecording.transcribeAndSummariseDescription':
    'Transcribe first, then write a summary from the text.',
  'postRecording.summarise': 'Summarise',
  'postRecording.summariseDescription':
    'Write a summary from the text already captured during the recording.',
  'postRecording.keep': 'Keep audio only',
  'postRecording.keepDescription':
    'No transcription or summary. You can do either later from the meeting.',
  'postRecording.working': 'Working on the recording. You can keep using the app while this runs.',
  'postRecording.finishing': 'Finishing up…',
  'postRecording.saved': 'Recording saved',
  'postRecording.savedDetail': 'Audio saved. Choose whether to transcribe it.',
  'postRecording.transcribing': 'Transcribing the recording…',
  'postRecording.transcribeKeepsRunning': 'You can keep using the app while this runs.',
  'postRecording.transcribeComplete': 'Transcription complete',
  'postRecording.transcribeFailed': 'Transcription failed',
  'postRecording.diskLowTitle': 'Low disk space',
  'postRecording.diskLowDetail':
    'Only {free} MB free where recordings are saved. Free up space so the rest of this recording is not lost.',
  'postRecording.limitApproaching': '{what}: stopping in {remaining}',
  'postRecording.maxLengthLabel': 'Maximum recording length',
  'postRecording.noAudioLabel': 'No audio detected',
  'postRecording.stoppedAtLimit': 'Recording stopped at the {duration} limit.',
  'postRecording.stoppedAfterSilence': 'Recording stopped after {duration} without audio.',

  'speakers.title': 'Speaker Separation',
  'speakers.description':
    'Labels who said what, so a meeting reads as a conversation rather than a wall of text. Runs locally after the recording stops; nothing is uploaded.',
  'speakers.tuning': 'Advanced',
  'speakers.tuningHint': 'only change these if the speaker count is wrong',
  'speakers.tuningChanged': 'These are no longer the calibrated defaults.',
  'speakers.threshold': 'Clustering threshold',
  'speakers.thresholdRange': '{min} to {max}',
  'speakers.thresholdHelp':
    'Higher merges more, producing fewer speakers. The calibrated value for the bundled model is {calibrated}. Lower it if one person is being split into several; raise it if several people are being merged.',
  'speakers.maxSpeakers': 'Maximum speakers',
  'speakers.maxSpeakersRange': '{min} to {max}',
  'speakers.maxSpeakersHelp':
    'A ceiling, not a target. It only has an effect when more speakers than this are found, in which case the closest ones are merged.',
  'speakers.tuningResetAction': 'Restore default',
  'speakers.tuningSaved': 'Separation settings saved',
  'speakers.tuningReset': 'Separation settings restored to the calibrated defaults',
  'speakers.tuningInvalid': 'Enter a number for both settings',
  'speakers.tuningLoadFailed': 'Could not read the separation settings',
  'speakers.tuningUnavailable': 'unavailable — click to retry',
  'speakers.tuningSaveFailed': 'Could not save the separation settings',  'speakers.label': 'Speaker {n}',
  'speakers.phaseSegmenting': 'Finding speech…',
  'speakers.phaseEmbedding': 'Identifying voices…',
  'speakers.phaseClustering': 'Grouping speakers…',
  'speakers.ready': 'Models ready',
  'speakers.missing': 'Models not downloaded',
  'speakers.download': 'Download models',
  'speakers.downloading': 'Downloading…',
  'speakers.redownload': 'Re-download',
  'speakers.size': 'About {size} MB',
  'speakers.folder': 'Models folder',
  'speakers.openFolder': 'Open folder',
  'speakers.checkFailed': 'Could not check the speaker separation models',
  'speakers.downloadFailed': 'Could not download the models',
  'speakers.downloadDone': 'Speaker separation models are ready',
  'speakers.run': 'Identify speakers',
  'speakers.running': 'Identifying speakers…',
  'speakers.runNeedsTranscript':
    'Transcribe this meeting first — speaker labels are attached to transcript segments.',
  'speakers.runHint':
    'Runs over the saved recording and labels each transcript segment. This can take a while on a long meeting.',
  'speakers.runDone': 'Identified {count} speaker(s)',
  'speakers.runNone': 'No distinct speakers could be identified',
  'speakers.runFailed': 'Could not identify speakers',
  'speakers.staleNote':
    'Re-transcribing a meeting keeps the existing speaker labels, matched by time.',
  'recordingLevel.waiting': 'Waiting for audio…',
  'recordingLevel.silent':
    'No audio is reaching{device}. Check the device is not muted and that it is the one actually playing or capturing the meeting.',
  'recordingLevel.quiet':
    'Audio level is very low{device}. Move closer to the microphone or raise the input gain.',
  'recordingLevel.clipping':
    'Audio is clipping{device}, which distorts speech and hurts transcription. Lower the input gain.',
};

const ZH_CN: Record<string, string> = {
  'common.cancel': '取消',
  'common.delete': '删除',
  'common.rename': '重命名',
  'common.refresh': '刷新',
  'common.reload': '重新加载',
  'common.add': '添加',
  'common.remove': '移除',
  'common.back': '返回',
  'common.settings': '设置',
  'common.loading': '加载中…',
  'common.unknownDate': '日期未知',
  'common.retry': '重试',

  'settings.title': '设置',
  'settings.tabs.general': '通用',
  'settings.tabs.recording': '录音',
  'settings.tabs.library': '录音库',
  'settings.tabs.dictionary': '词典',
  'settings.tabs.transcription': '转写',
  'settings.tabs.summary': '纪要',
  'settings.tabs.beta': '实验功能',
  'settings.language': '界面语言',
  'settings.language.description':
    '按钮、标签和提示所用的语言。与转写和纪要使用的语言是两回事。',

  'dictionary.title': '自定义词典',
  'dictionary.description':
    '语音模型应当精确拼写的专有名词：人名、产品名、行业术语。这些词会同时传给转写清理和纪要生成，因此能保留到你的笔记里。',
  'dictionary.correctSpelling': '正确写法',
  'dictionary.misheardAs': '常被听成',
  'dictionary.optional': '（可选）',
  'dictionary.termPlaceholder': '例如：张伟',
  'dictionary.misheardPlaceholder': '例如：张伟伟',
  'dictionary.addHint': '填上"常被听成"的形式，纠正效果比只列词条强得多。',
  'dictionary.empty': '还没有词条。把老是转错的名字加进来。',
  'dictionary.count': '已用 {count} / {max} 条。离开输入框时自动保存。',
  'dictionary.limitReached': '词典最多 {max} 条',
  'dictionary.loadFailed': '无法加载词典',
  'dictionary.saveFailed': '无法保存词典',
  'dictionary.removeTitle': '移除此词条',

  'nav.home': '首页',
  'nav.library': '会议与录音',
  'nav.settings': '设置',
  'nav.about': '关于',
  'nav.searchPlaceholder': '搜索会议内容…',
  'nav.importAudio': '导入音频',
  'nav.startRecording': '开始录音',
  'nav.recordingInProgress': '正在录音…',
  'nav.searching': '搜索中…',
  'nav.emptyLibrary': '还没有会议或录音。录制之后会出现在这里。',
  'nav.noMatches': '没有匹配「{query}」的内容',
  'nav.openMeeting': '打开「{title}」的会议详情',
  'nav.openRecording': '在录音库中打开「{title}」',
  'nav.openNotes': '打开「{title}」的笔记',
  'nav.renameMeeting': '重命名「{title}」',
  'nav.deleteMeeting': '删除「{title}」',
  'nav.match': '命中：',
  'common.save': '保存',

  'recordingsPage.title': '录音文件',
  'recordingsPage.description': '磁盘上保存的全部录音。可以逐个转写、生成纪要、重命名、移动或删除。',

  'library.title': '录音库',
  'library.description':
    '磁盘上保存的全部录音，最新的在前。包含没有对应会议记录的录音，所以不会有任何遗漏。',
  'library.loading': '正在读取录音目录…',
  'library.folder': '目录',
  'library.summary': '{count} 个录音 · 共 {size}',
  'library.folderMissing': '该目录还不存在。录制第一段录音时会自动创建。',
  'library.empty': '该目录下没有找到录音。',
  'library.loadFailed': '无法读取录音目录',
  'library.openFolder': '打开该录音所在目录',
  'library.renameTitle': '重命名该录音',
  'library.moveTitle': '移动到其它位置',
  'library.transcribe': '转写',
  'library.transcribeTitle': '转写该录音',
  'library.summarise': '生成纪要',
  'library.resummarise': '重新生成纪要',
  'library.summariseTitle': '根据转写生成纪要',
  'library.removeSummary': '移除纪要',
  'library.removeSummaryTitle': '移除纪要，保留转写',
  'library.deleteTitle': '删除该录音及其音频',
  'library.badgeNoAudio': '无音频',
  'library.badgeIncomplete': '未完成',
  'library.badgeTranscript': '转写',
  'library.badgeSummary': '纪要',
  'library.noAudioWarning': '该目录下没有找到音频文件。',
  'library.footnote':
    '删除会移除整个录音目录，包括音频。移动会让它离开本列表，因为它已经不在录音目录里了。',
  'library.renameDialogTitle': '重命名录音',
  'library.namePlaceholder': '录音名称',
  'library.deleteDialogTitle': '删除该录音？',
  'library.deleteDialogBody': '将从磁盘移除「{name}」及其音频。此操作无法撤销。',
  'library.operationFailed': '操作失败',
  'library.transcribeStarted': '已开始转写',
  'library.transcribeStartedDetail': '转写期间可以继续使用应用。',
  'library.transcribeFailed': '无法开始转写',

  'recording.limitsTitle': '录音安全限制',
  'recording.limitsDescription': '防止录音一直开着忘记停止。两项默认都关闭。',
  'recording.silenceStop': '静音后自动停止',
  'recording.silenceStopHint':
    '适合无人值守的会议。安静的结尾可能会被截断，建议用最短的可选时长。',
  'recording.maxLength': '最长录音时长',
  'recording.maxLengthHint': '达到该时长后自动停止，忘关的录音也不会写满磁盘。',
  'recording.retentionTitle': '录音保留策略',
  'recording.retentionDescription':
    '自动删除超过指定天数的录音。默认关闭，除非你主动选择，否则不会删除任何录音。',
  'recording.keepFor': '保留时长',
  'recording.forever': '永久保留',
  'recording.days': '{days} 天',
  'recording.previewPurge': '预览将被删除的录音',
  'recording.checking': '检查中…',
  'recording.recordingFormat': '录音格式',
  'recording.audioQuality': '音质',
  'recording.formatHint': '录音以 {file} 保存在各自的会议目录中。',
  'recording.qualityHint': '音质越高文件越大。语音录音通常选"中"就够。',
  'recording.losslessNote': '{format} 是无损格式，音质设置对它无效。',
  'recording.lockedWhileRecording': '录音进行中，格式与音质已锁定。',
  'recording.silenceOf': '静音 {duration}',
  'recording.selectFormat': '选择格式',
  'recording.selectQuality': '选择音质',
  'recording.cleanupTitle': '转写文本整理',
  'recording.cleanupToggle': '录音结束后整理转写文本',
  'recording.cleanupDescription':
    '用本地模型重写最终转写文本：删除口语填充词、修正口述标点、把说出来的数字和日期改成规范写法。默认关闭，因为它会改写你的原话。只使用本地模型，转写内容不会离开这台机器。',
  'recording.cleanupLocalOnly':
    '需要本地模型（Ollama 或内置模型）。若纪要配置的是云端 provider，将跳过整理，而不是上传你的转写文本。',

  'postRecording.title': '录音已结束',
  'postRecording.deferredMessage': '本次录音设置了延后转写。请选择接下来怎么处理。',
  'postRecording.noSpeechMessage': '没有捕获到可用于转写的语音。请选择如何处理音频。',
  'postRecording.hasTranscriptMessage': '转写已完成。需要生成会议纪要吗？',
  'postRecording.transcribe': '转写',
  'postRecording.transcribeDescription': '把录音转成文字，使用当前配置的语音模型。',
  'postRecording.transcribeAndSummarise': '转写并生成纪要',
  'postRecording.transcribeAndSummariseDescription': '先转写，再根据文字生成纪要。',
  'postRecording.summarise': '生成纪要',
  'postRecording.summariseDescription': '根据录音过程中已捕获的文字生成纪要。',
  'postRecording.keep': '仅保留音频',
  'postRecording.keepDescription': '不转写也不生成纪要。之后可以在会议页面里操作。',
  'postRecording.working': '正在处理录音。期间可以继续使用应用。',
  'postRecording.finishing': '正在收尾…',
  'postRecording.saved': '录音已保存',
  'postRecording.savedDetail': '音频已保存。请选择是否转写。',
  'postRecording.transcribing': '正在转写录音…',
  'postRecording.transcribeKeepsRunning': '转写期间可以继续使用应用。',
  'postRecording.transcribeComplete': '转写完成',
  'postRecording.transcribeFailed': '转写失败',
  'postRecording.diskLowTitle': '磁盘空间不足',
  'postRecording.diskLowDetail':
    '录音保存位置仅剩 {free} MB。请清理空间，以免本次录音的后续部分丢失。',
  'postRecording.limitApproaching': '{what}：{remaining} 后停止',
  'postRecording.maxLengthLabel': '已达到最长录音时长',
  'postRecording.noAudioLabel': '未检测到音频',
  'postRecording.stoppedAtLimit': '录音已达到 {duration} 上限并停止。',
  'postRecording.stoppedAfterSilence': '已连续 {duration} 没有音频，录音停止。',

  'speakers.title': '说话人分离',
  'speakers.description':
    '标注每句话是谁说的，让会议记录读起来像对话而不是一整片文字。录音结束后在本地运行，不会上传任何内容。',
  'speakers.tuning': '高级设置',
  'speakers.tuningHint': '仅在说话人数量不对时调整',
  'speakers.tuningChanged': '当前值已不是校准默认值。',
  'speakers.threshold': '聚类阈值',
  'speakers.thresholdRange': '{min} 到 {max}',
  'speakers.thresholdHelp':
    '值越高合并越多，说话人越少。内置模型的校准值为 {calibrated}。如果一个人被拆成好几个，就调低；如果好几个人被合并成一个，就调高。',
  'speakers.maxSpeakers': '说话人上限',
  'speakers.maxSpeakersRange': '{min} 到 {max}',
  'speakers.maxSpeakersHelp':
    '这是上限而不是目标值。只有识别出的说话人超过这个数时才起作用，此时最接近的会被合并。',
  'speakers.tuningResetAction': '恢复默认',
  'speakers.tuningSaved': '说话人分离设置已保存',
  'speakers.tuningReset': '已恢复为校准默认值',
  'speakers.tuningInvalid': '两项设置都需要填写数字',
  'speakers.tuningLoadFailed': '无法读取说话人分离设置',
  'speakers.tuningUnavailable': '不可用 —— 点击重试',
  'speakers.tuningSaveFailed': '无法保存说话人分离设置',  'speakers.label': '说话人 {n}',
  'speakers.phaseSegmenting': '正在找出语音…',
  'speakers.phaseEmbedding': '正在识别声纹…',
  'speakers.phaseClustering': '正在归组说话人…',
  'speakers.ready': '模型已就绪',
  'speakers.missing': '模型尚未下载',
  'speakers.download': '下载模型',
  'speakers.downloading': '下载中…',
  'speakers.redownload': '重新下载',
  'speakers.size': '约 {size} MB',
  'speakers.folder': '模型目录',
  'speakers.openFolder': '打开目录',
  'speakers.checkFailed': '无法检查说话人分离模型',
  'speakers.downloadFailed': '模型下载失败',
  'speakers.downloadDone': '说话人分离模型已就绪',
  'speakers.run': '识别说话人',
  'speakers.running': '正在识别说话人…',
  'speakers.runNeedsTranscript': '请先转写这个会议 —— 说话人标签是附加在转写段落上的。',
  'speakers.runHint': '在已保存的录音上运行，为每段转写标注说话人。会议较长时可能需要一些时间。',
  'speakers.runDone': '识别出 {count} 位说话人',
  'speakers.runNone': '未能识别出不同的说话人',
  'speakers.runFailed': '说话人识别失败',
  'speakers.staleNote': '重新转写会议时会保留已有的说话人标签，按时间对应。',
  'recordingLevel.waiting': '等待音频…',
  'recordingLevel.silent':
    '{device} 没有收到音频。请确认设备未被静音，且它确实是正在播放或采集会议的那个设备。',
  'recordingLevel.quiet': '{device} 音量过低。请靠近麦克风或调高输入增益。',
  'recordingLevel.clipping': '{device} 出现削波，会失真并影响转写。请调低输入增益。',
};

const TABLES: Record<Locale, Record<string, string>> = {
  en: EN,
  'zh-CN': ZH_CN,
};

/** Values substituted into `{placeholder}` tokens. */
export type TranslateParams = Record<string, string | number>;

/**
 * Look up `key` in `locale`, substituting `params`.
 *
 * Falls back to English, then to the key itself. Returning the key rather than
 * an empty string means a missing translation is visible in the UI instead of
 * silently collapsing the label to nothing.
 */
export function translate(locale: Locale, key: string, params?: TranslateParams): string {
  const template = TABLES[locale]?.[key] ?? EN[key] ?? key;

  if (!params) return template;

  return template.replace(/\{(\w+)\}/g, (match, name: string) => {
    const value = params[name];
    return value === undefined ? match : String(value);
  });
}

/** Keys the given locale cannot translate yet, for completeness checks. */
export function missingKeys(locale: Locale): string[] {
  const table = TABLES[locale] ?? {};
  return Object.keys(EN).filter((key) => !(key in table));
}

/** Normalise a stored or detected value to a supported locale. */
export function resolveLocale(raw: string | null | undefined): Locale {
  if (!raw) return DEFAULT_LOCALE;

  const value = raw.trim();
  if (LOCALES.includes(value as Locale)) return value as Locale;

  // Accept a bare language tag so a stored "zh" or a browser "zh-CN" both land
  // on the Chinese table.
  const lower = value.toLowerCase();
  if (lower.startsWith('zh')) return 'zh-CN';
  if (lower.startsWith('en')) return 'en';

  return DEFAULT_LOCALE;
}

/** Read the persisted choice, or the default. */
export function readStoredLocale(): Locale {
  if (typeof window === 'undefined') return DEFAULT_LOCALE;

  try {
    return resolveLocale(window.localStorage.getItem(LOCALE_STORAGE_KEY));
  } catch {
    // Storage can be unavailable; the default is a fine answer.
    return DEFAULT_LOCALE;
  }
}

/** Persist the choice. */
export function storeLocale(locale: Locale): void {
  if (typeof window === 'undefined') return;

  try {
    window.localStorage.setItem(LOCALE_STORAGE_KEY, locale);
  } catch {
    // Non-fatal: the choice simply does not survive a restart.
  }
}
