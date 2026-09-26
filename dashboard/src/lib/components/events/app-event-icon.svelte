<!-- AppEventIcon — the icon and colour for one kind of app event. -->
<script lang="ts">
	import type { AppEvent } from '$lib/api';
	import { cn } from '$lib/utils';
	import RocketIcon from '~icons/ri/rocket-2-line';
	import CheckIcon from '~icons/ri/checkbox-circle-line';
	import CloseIcon from '~icons/ri/close-circle-line';
	import SkipIcon from '~icons/ri/skip-forward-line';
	import RestartIcon from '~icons/ri/restart-line';
	import BugIcon from '~icons/ri/error-warning-line';
	import MemoryIcon from '~icons/ri/ram-line';
	import LockIcon from '~icons/ri/lock-line';
	import LockUnlockIcon from '~icons/ri/lock-unlock-line';
	import GlobeIcon from '~icons/ri/global-line';
	import CpuIcon from '~icons/ri/cpu-line';
	import SettingsIcon from '~icons/ri/settings-3-line';
	import PowerIcon from '~icons/ri/shut-down-line';
	import AlarmIcon from '~icons/ri/alarm-warning-line';
	import BellIcon from '~icons/ri/notification-off-line';

	let { event, class: className }: { event: AppEvent; class?: string } = $props();

	const ICONS = {
		deploy_started: RocketIcon,
		deploy_succeeded: CheckIcon,
		deploy_failed: CloseIcon,
		deploy_cancelled: SkipIcon,
		restart: RestartIcon,
		crash: BugIcon,
		oom: MemoryIcon,
		cert_issued: LockIcon,
		cert_failed: LockUnlockIcon,
		domain_changed: GlobeIcon,
		resources_changed: CpuIcon,
		config_changed: SettingsIcon,
		control: PowerIcon,
		alert_fired: AlarmIcon,
		alert_resolved: BellIcon
	} as const;

	const TONES = {
		info: 'bg-muted text-muted-foreground',
		success: 'bg-success/10 text-success',
		warning: 'bg-warn/10 text-warn',
		danger: 'bg-destructive/10 text-destructive'
	} as const;

	let Icon = $derived(ICONS[event.kind] ?? SettingsIcon);
</script>

<span
	class={cn(
		'inline-flex size-7 shrink-0 items-center justify-center rounded-full',
		TONES[event.severity] ?? TONES.info,
		className
	)}
	aria-hidden="true"
>
	<Icon class="size-4" />
</span>
