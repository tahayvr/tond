<script lang="ts">
	import CheckCircle from '~icons/ri/checkbox-circle-line';
	import type { Component } from 'svelte';

	interface Step {
		id: string;
		label: string;
		icon: Component;
	}

	interface Props {
		steps: Step[];
		currentStep: number;
		onStepClick: (index: number) => void;
	}

	let { steps, currentStep, onStepClick }: Props = $props();
</script>

<div class="relative">
	<div class="flex items-center justify-between">
		{#each steps as step, index (step.id)}
			<div class="flex flex-1 items-center">
				<button
					class="group flex flex-col items-center gap-2"
					onclick={() => onStepClick(index)}
					disabled={index > currentStep}
					aria-label="Step {index + 1}: {step.label}"
					aria-current={index === currentStep ? 'step' : undefined}
				>
					<div
						class="flex size-9 items-center justify-center rounded-sm border-2 font-mono text-sm font-bold transition-all sm:size-10 {index ===
						currentStep
							? 'border-primary bg-primary text-primary-foreground'
							: index < currentStep
								? 'border-chart-2 bg-chart-2/20 text-chart-2'
								: 'border-border bg-transparent text-muted-foreground'}"
					>
						{#if index < currentStep}
							<CheckCircle class="h-5 w-5" />
						{:else}
							<step.icon class="h-5 w-5" />
						{/if}
					</div>
					<span
						class="hidden font-mono text-[10px] tracking-wider uppercase transition-colors sm:inline {index ===
						currentStep
							? 'text-foreground'
							: index < currentStep
								? 'text-chart-2'
								: 'text-muted-foreground'}"
					>
						{step.label}
					</span>
				</button>
				{#if index < steps.length - 1}
					<div
						class="mx-1 h-0.5 flex-1 transition-colors sm:mx-2 {index < currentStep
							? 'bg-chart-2'
							: 'bg-border'}"
					></div>
				{/if}
			</div>
		{/each}
	</div>
	<!-- Phones show icons only; name the current step in words. -->
	<p class="mt-2 font-mono text-xs text-muted-foreground sm:hidden">
		Step {currentStep + 1} of {steps.length}:
		<span class="text-foreground">{steps[currentStep]?.label}</span>
	</p>
</div>
