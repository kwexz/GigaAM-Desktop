declare module '@toast-ui/editor' {
	export default class Editor {
		constructor(options: { el: HTMLElement; initialValue: string; initialEditType: 'wysiwyg'; previewStyle: 'tab'; height: string; usageStatistics: boolean; hideModeSwitch: boolean; theme?: string; toolbarItems: string[][] });
		on(event: 'change', handler: () => void): void;
		getMarkdown(): string;
		destroy(): void;
	}
}
