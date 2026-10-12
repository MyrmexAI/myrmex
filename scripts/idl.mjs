// Shank IDL для всех программ: idl/<prog>.json. Адреса берутся из keys/program-ids.txt.
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

const ids = readFileSync('keys/program-ids.txt', 'utf8').trim().split('\n').map((l) => l.trim().split(/\s+/));
for (const [name, id] of ids) {
  execFileSync('shank', ['idl', '-r', `programs/${name}`, '-o', 'idl', '-p', id], { stdio: ['ignore', 'ignore', 'inherit'] });
  console.log(`idl/${name}.json`);
}
