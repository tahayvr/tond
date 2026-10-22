import { describe, expect, it } from 'vitest';
import {
	deploymentServiceBadge,
	formatCpu,
	formatMemory,
	isPlausibleDomain,
	normalizeDomain,
	parsePort,
	portChoices,
	serviceBadge,
	servicesSummary,
	shortDigest,
	suggestWebService
} from './compose';
import type { ComposeService } from './api';

function svc(over: Partial<ComposeService>): ComposeService {
	return {
		name: 'web',
		image: 'nginx',
		builds: false,
		ports: [80],
		has_healthcheck: false,
		published_port: 80,
		host_port: 8300,
		redeploy_required: false,
		domains: [],
		status: 'running',
		health: null,
		exit_code: null,
		container_id: 'abc',
		running_image: null,
		image_digest: null,
		cpu: null,
		mem: null,
		...over
	};
}

describe('serviceBadge', () => {
	it('refines running with the healthcheck', () => {
		expect(serviceBadge(svc({})).label).toBe('Running');
		expect(serviceBadge(svc({ health: 'healthy' })).status).toBe('healthy');
		expect(serviceBadge(svc({ health: 'unhealthy' })).status).toBe('unhealthy');
		expect(serviceBadge(svc({ health: 'starting' })).label).toBe('Starting');
	});
	it('treats a clean exit as done and a crash as failed', () => {
		expect(serviceBadge(svc({ status: 'exited', exit_code: 0 }))).toMatchObject({
			label: 'Done',
			tone: 'neutral'
		});
		expect(serviceBadge(svc({ status: 'exited', exit_code: 137 }))).toMatchObject({
			status: 'failed',
			label: 'Exited (137)'
		});
		expect(serviceBadge(svc({ status: 'missing' })).label).toBe('Not running');
		expect(serviceBadge(svc({ status: 'pending' })).label).toBe('Not deployed');
		expect(serviceBadge(svc({ status: 'parked' }))).toEqual({
			status: 'parked',
			label: 'Parked',
			tone: 'neutral'
		});
	});
});

describe('servicesSummary', () => {
	it('counts running and finished services', () => {
		expect(servicesSummary([])).toBe('No services');
		expect(servicesSummary([svc({})])).toBe('1 service running');
		expect(servicesSummary([svc({}), svc({ name: 'init', status: 'exited', exit_code: 0 })])).toBe(
			'All 2 services running'
		);
		expect(servicesSummary([svc({}), svc({ name: 'db', status: 'exited', exit_code: 1 })])).toBe(
			'1 of 2 running'
		);
		const parked = svc({ name: 'old', status: 'parked', parked: true });
		expect(servicesSummary([svc({}), parked])).toBe('1 service running · 1 parked');
		expect(servicesSummary([parked])).toBe('No services running · 1 parked');
	});
});

describe('formatting', () => {
	it('formats memory and cpu', () => {
		expect(formatMemory(null)).toBe('—');
		expect(formatMemory(5 * 1_048_576)).toBe('5.0 MiB');
		expect(formatMemory(84 * 1_048_576)).toBe('84 MiB');
		expect(formatMemory(1536 * 1_048_576)).toBe('1.5 GiB');
		expect(formatCpu(null)).toBe('—');
		expect(formatCpu(125)).toBe('1.3%');
		expect(formatCpu(1250)).toBe('13%');
	});
	it('shortens digests', () => {
		expect(shortDigest('redis@sha256:4f1c2a3b9e01aaaaaaaa')).toBe('4f1c2a3b9e01');
		expect(shortDigest('sha256:0123456789abcdef')).toBe('0123456789ab');
		expect(shortDigest(null)).toBeNull();
	});
});

describe('ports and domains', () => {
	it('offers declared ports plus the current choice', () => {
		expect(portChoices({ ports: [8080, 80], published_port: 3000 })).toEqual([80, 3000, 8080]);
		expect(portChoices({ ports: [80], published_port: null })).toEqual([80]);
	});
	it('parses ports', () => {
		expect(parsePort(' 8080 ')).toBe(8080);
		expect(parsePort('0')).toBeNull();
		expect(parsePort('70000')).toBeNull();
		expect(parsePort('80a')).toBeNull();
	});
	it('normalises and checks domains', () => {
		expect(normalizeDomain(' https://Shop.Example.com/path ')).toBe('shop.example.com');
		expect(normalizeDomain('api.example.com:443')).toBe('api.example.com');
		expect(normalizeDomain('x.example.com.')).toBe('x.example.com');
		expect(isPlausibleDomain('shop.example.com')).toBe(true);
		expect(isPlausibleDomain('localhost')).toBe(false);
		expect(isPlausibleDomain('bad_host.example.com')).toBe(false);
	});
	it('suggests the web service', () => {
		const specs = [
			{ name: 'db', image: 'postgres', builds: false, ports: [], has_healthcheck: true },
			{ name: 'api', image: null, builds: true, ports: [3000], has_healthcheck: false },
			{ name: 'web', image: 'nginx', builds: false, ports: [80], has_healthcheck: false }
		];
		expect(suggestWebService(specs, 80)?.name).toBe('web');
		expect(suggestWebService(specs, 1234)?.name).toBe('api');
		expect(suggestWebService([specs[0]], 80)).toBeNull();
	});
});

describe('deploymentServiceBadge', () => {
	it('names each step', () => {
		expect(deploymentServiceBadge({ status: 'pulling', source: 'pull' }).label).toBe('Pulling');
		expect(deploymentServiceBadge({ status: 'ready', source: 'build' }).label).toBe('Built');
		expect(deploymentServiceBadge({ status: 'ready', source: 'reuse' }).label).toBe('Image kept');
		expect(deploymentServiceBadge({ status: 'healthy', source: 'pull' }).label).toBe('Ready');
		expect(deploymentServiceBadge({ status: 'pending', source: 'pull' }).label).toBe('Waiting');
	});
});
