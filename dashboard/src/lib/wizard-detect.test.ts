import { describe, expect, it } from 'vitest';
import {
	defaultMemoryCap,
	formatBytes,
	parseComposePorts,
	parseDockerfileExpose
} from './wizard-detect';

describe('parseDockerfileExpose', () => {
	it('reads TCP ports from EXPOSE lines', () => {
		expect(
			parseDockerfileExpose('FROM nginx\nEXPOSE 80\nexpose 443/tcp 53/udp\nEXPOSE 80\nCMD x')
		).toEqual([80, 443]);
		expect(parseDockerfileExpose('FROM node\nCMD ["node", "server.js"]')).toEqual([]);
	});
});

describe('parseComposePorts', () => {
	it('reads short, long and flow syntax', () => {
		const yaml = `services:
  web:
    image: nginx
    ports:
      - "8080:80"
      - 127.0.0.1:9443:443/tcp
      - "53:53/udp"
  api:
    image: node
    expose:
      - 3000
    environment:
      - PORT=3000
  admin:
    ports: ["8081:8000"]
  long:
    ports:
      - target: 9000
        published: 9001
`;
		expect(parseComposePorts(yaml)).toEqual([80, 443, 3000, 8000, 9000]);
	});

	it('ignores list items outside ports/expose', () => {
		expect(parseComposePorts('services:\n  a:\n    command:\n      - "80"\n')).toEqual([]);
	});
});

describe('memory cap', () => {
	it('mirrors the server default', () => {
		expect(defaultMemoryCap(4 * 1024 ** 3)).toBe(3 * 1024 ** 3);
		expect(defaultMemoryCap(0)).toBe(256 * 1024 ** 2);
		expect(formatBytes(3 * 1024 ** 3)).toBe('3 GB');
		expect(formatBytes(1.5 * 1024 ** 3)).toBe('1.5 GB');
		expect(formatBytes(512 * 1024 ** 2)).toBe('512 MB');
	});
});
