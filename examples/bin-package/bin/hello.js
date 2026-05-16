#!/usr/bin/env node

const { Command } = require("commander");

const program = new Command();

program
  .name("orix-example-hello")
  .option("-n, --name <name>", "name to greet", "orix")
  .action((options) => {
    console.log(`hello, ${options.name}`);
  });

program.parse();
