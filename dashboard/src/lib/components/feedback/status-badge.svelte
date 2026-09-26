<!--
  StatusBadge — the only way to show a resource status.

  <StatusBadge status={app.status} kind="app" />
  <StatusBadge status={run.status} kind="run" size="sm" />
  <StatusBadge status="custom" tone="warning" label="Needs attention" />

  Colour + icon + text; the tone comes from `describeStatus` in $lib/status.
-->
<script lang="ts">
	import CheckIcon from '~icons/ri/checkbox-circle-fill';
	import SpinnerIcon from '~icons/ri/loader-4-line';
	import WarningIcon from '~icons/ri/alert-fill';
	import ErrorIcon from '~icons/ri/close-circle-fill';
	import StopIcon from '~icons/ri/stop-circle-line';
	import PauseIcon from '~icons/ri/pause-circle-line';
	import DotIcon from '~icons/ri/checkbox-blank-circle-fill';
	import {
		describeStatus,
		TONE_CLASSES,
		type StatusKind,
		type StatusTone,
		type StatusIcon
	} from '$lib/status';
	import { cn } from '$lib/utils';

	let {
		status,
		kind = 'generic',
		tone,
		label,
		size = 'default',
		class: className
	}: {
		status: string | null | undefined;
		kind?: StatusKind;
		/** Override the tone derived from `status`. */
		tone?: StatusTone;
		/** Override the label derived from `status`. */
		label?: string;
		size?: 'sm' | 'default';
		class?: string;
	} = $props();

	let described = $derived(describeStatus(status, kind));
	let finalTone = $derived(tone ?? described.tone);
	let finalLabel = $derived(label ?? described.label);

	const ICONS: Record<StatusIcon, typeof CheckIcon> = {
		check: CheckIcon,
		spinner: SpinnerIcon,
		warning: WarningIcon,
		error: ErrorIcon,
		stop: StopIcon,
		pause: PauseIcon,
		dot: DotIcon
	};
	let Icon = $derived(ICONS[described.icon]);
</script>

<span
	data-slot="status-badge"
	data-tone={finalTone}
	class={cn(
		'inline-flex w-fit shrink-0 items-center gap-1 rounded-sm border font-medium whitespace-nowrap',
		size === 'sm' ? 'px-1.5 py-0 text-[11px] leading-5' : 'px-2 py-0.5 text-xs',
		TONE_CLASSES[finalTone],
		className
	)}
>
	<Icon
		aria-hidden="true"
		class={cn(
			size === 'sm' ? 'size-3' : 'size-3.5',
			described.icon === 'spinner' && 'animate-spin motion-reduce:animate-none',
			described.icon === 'dot' && 'size-2'
		)}
	/>
	{finalLabel}
</span>
