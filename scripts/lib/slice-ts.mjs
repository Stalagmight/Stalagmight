#!/usr/bin/env node
/**
 * slice-ts.mjs - Extract code from TypeScript/JavaScript files using AST parsing
 *
 * Uses the TypeScript compiler API for accurate parsing of TS/TSX/JS/JSX files.
 *
 * Usage:
 *   node slice-ts.mjs <file> --toc              # Show table of contents
 *   node slice-ts.mjs <file> --fn <name>        # Extract function/method
 *   node slice-ts.mjs <file> --class <name>     # Extract class
 *   node slice-ts.mjs <file> --interface <name> # Extract interface/type
 *   node slice-ts.mjs <file> --component <name> # Extract React component
 *   node slice-ts.mjs <file> --lines <start>-<end> # Extract line range
 */

import * as fs from 'fs';
import * as path from 'path';
import * as ts from 'typescript';

const args = process.argv.slice(2);
if (args.length < 2) {
  console.error('Usage: slice-ts.mjs <file> --toc | --fn <name> | --class <name> | --interface <name> | --component <name> | --lines <range>');
  process.exit(1);
}

const filePath = args[0];
const mode = args[1];
const target = args[2];

if (!fs.existsSync(filePath)) {
  // Not the path itself: it is the caller's, and can name their own
  // directories outside the checkout (#576 W51).
  console.error('File not found: <file>');
  process.exit(1);
}

const sourceText = fs.readFileSync(filePath, 'utf-8');
const ext = path.extname(filePath).toLowerCase();

// Determine script kind
let scriptKind;
switch (ext) {
  case '.tsx':
    scriptKind = ts.ScriptKind.TSX;
    break;
  case '.ts':
    scriptKind = ts.ScriptKind.TS;
    break;
  case '.jsx':
    scriptKind = ts.ScriptKind.JSX;
    break;
  case '.js':
  case '.mjs':
  case '.cjs':
    scriptKind = ts.ScriptKind.JS;
    break;
  default:
    scriptKind = ts.ScriptKind.Unknown;
}

const sourceFile = ts.createSourceFile(
  filePath,
  sourceText,
  ts.ScriptTarget.Latest,
  true,
  scriptKind
);

/**
 * Get line number (1-indexed) from position
 */
function getLine(pos) {
  return sourceFile.getLineAndCharacterOfPosition(pos).line + 1;
}

/**
 * Check if node has export modifier
 */
function isExported(node) {
  if (!node.modifiers) return false;
  return node.modifiers.some(m =>
    m.kind === ts.SyntaxKind.ExportKeyword ||
    m.kind === ts.SyntaxKind.DefaultKeyword
  );
}

/**
 * Check if node has async modifier
 */
function isAsync(node) {
  if (!node.modifiers) return false;
  return node.modifiers.some(m => m.kind === ts.SyntaxKind.AsyncKeyword);
}

/**
 * Extract text from source with line numbers
 */
function extractWithLineNumbers(startPos, endPos) {
  const startLine = getLine(startPos) - 1;  // Convert to 0-indexed
  const endLine = getLine(endPos) - 1;
  const lines = sourceText.split('\n');
  const extracted = lines.slice(startLine, endLine + 1);

  const lineNumWidth = String(endLine + 2).length;  // +2 for 1-indexed display
  return extracted.map((line, i) => {
    const lineNum = String(startLine + i + 1).padStart(lineNumWidth);
    return `${lineNum}| ${line}`;
  }).join('\n');
}

/**
 * Collect declarations by iterating top-level statements
 */
function collectDeclarations() {
  const declarations = [];

  for (const statement of sourceFile.statements) {
    collectFromNode(statement, declarations, null);
  }

  return declarations;
}

function collectFromNode(node, declarations, parentClass) {
  const startLine = getLine(node.getStart());
  const endLine = getLine(node.getEnd());

  // Function declaration: function foo() {}
  if (ts.isFunctionDeclaration(node) && node.name) {
    declarations.push({
      type: 'function',
      name: node.name.text,
      startLine,
      endLine,
      exported: isExported(node),
      async: isAsync(node),
      parent: parentClass
    });
    return;
  }

  // Class declaration: class Foo {}
  if (ts.isClassDeclaration(node) && node.name) {
    declarations.push({
      type: 'class',
      name: node.name.text,
      startLine,
      endLine,
      exported: isExported(node),
      async: false,
      parent: null
    });

    // Collect methods
    for (const member of node.members) {
      if (ts.isMethodDeclaration(member) && member.name) {
        const methodName = ts.isIdentifier(member.name) ? member.name.text :
                          ts.isPrivateIdentifier(member.name) ? member.name.text :
                          member.name.getText(sourceFile);
        declarations.push({
          type: 'method',
          name: methodName,
          startLine: getLine(member.getStart()),
          endLine: getLine(member.getEnd()),
          exported: false,
          async: isAsync(member),
          parent: node.name.text
        });
      }
      // Constructor
      if (ts.isConstructorDeclaration(member)) {
        declarations.push({
          type: 'method',
          name: 'constructor',
          startLine: getLine(member.getStart()),
          endLine: getLine(member.getEnd()),
          exported: false,
          async: false,
          parent: node.name.text
        });
      }
      // Getters/setters
      if (ts.isGetAccessorDeclaration(member) && member.name) {
        const name = ts.isIdentifier(member.name) ? member.name.text : member.name.getText(sourceFile);
        declarations.push({
          type: 'method',
          name: `get ${name}`,
          startLine: getLine(member.getStart()),
          endLine: getLine(member.getEnd()),
          exported: false,
          async: false,
          parent: node.name.text
        });
      }
      if (ts.isSetAccessorDeclaration(member) && member.name) {
        const name = ts.isIdentifier(member.name) ? member.name.text : member.name.getText(sourceFile);
        declarations.push({
          type: 'method',
          name: `set ${name}`,
          startLine: getLine(member.getStart()),
          endLine: getLine(member.getEnd()),
          exported: false,
          async: false,
          parent: node.name.text
        });
      }
    }
    return;
  }

  // Interface declaration
  if (ts.isInterfaceDeclaration(node)) {
    declarations.push({
      type: 'interface',
      name: node.name.text,
      startLine,
      endLine,
      exported: isExported(node),
      async: false,
      parent: null
    });
    return;
  }

  // Type alias
  if (ts.isTypeAliasDeclaration(node)) {
    declarations.push({
      type: 'type',
      name: node.name.text,
      startLine,
      endLine,
      exported: isExported(node),
      async: false,
      parent: null
    });
    return;
  }

  // Enum declaration
  if (ts.isEnumDeclaration(node)) {
    declarations.push({
      type: 'enum',
      name: node.name.text,
      startLine,
      endLine,
      exported: isExported(node),
      async: false,
      parent: null
    });
    return;
  }

  // Variable statement with function/arrow: const foo = () => {}
  if (ts.isVariableStatement(node)) {
    for (const decl of node.declarationList.declarations) {
      if (decl.name && ts.isIdentifier(decl.name) && decl.initializer) {
        // Arrow function or function expression
        if (ts.isArrowFunction(decl.initializer) || ts.isFunctionExpression(decl.initializer)) {
          // Check if it's a React component (PascalCase name)
          const name = decl.name.text;
          const isComponent = /^[A-Z]/.test(name);

          declarations.push({
            type: isComponent ? 'component' : 'function',
            name: name,
            startLine,
            endLine,
            exported: isExported(node),
            async: decl.initializer.modifiers?.some(m => m.kind === ts.SyntaxKind.AsyncKeyword) || false,
            parent: null
          });
        }
      }
    }
    return;
  }

  // Export default class Foo {} or export default function foo() {}
  if (ts.isExportAssignment(node) && node.expression) {
    if (ts.isClassExpression(node.expression) && node.expression.name) {
      declarations.push({
        type: 'class',
        name: node.expression.name.text,
        startLine,
        endLine,
        exported: true,
        async: false,
        parent: null
      });
    }
  }
}

/**
 * Find node in AST by name
 */
function findNodeByName(name, type = null) {
  let found = null;

  function visit(node) {
    if (found) return;

    // Function declaration
    if (ts.isFunctionDeclaration(node) && node.name?.text === name) {
      if (!type || type === 'function') {
        found = node;
        return;
      }
    }

    // Class declaration
    if (ts.isClassDeclaration(node) && node.name?.text === name) {
      if (!type || type === 'class') {
        found = node;
        return;
      }
    }

    // Interface
    if (ts.isInterfaceDeclaration(node) && node.name.text === name) {
      if (!type || type === 'interface') {
        found = node;
        return;
      }
    }

    // Type alias
    if (ts.isTypeAliasDeclaration(node) && node.name.text === name) {
      if (!type || type === 'interface' || type === 'type') {
        found = node;
        return;
      }
    }

    // Enum
    if (ts.isEnumDeclaration(node) && node.name.text === name) {
      if (!type || type === 'enum') {
        found = node;
        return;
      }
    }

    // Variable with function
    if (ts.isVariableStatement(node)) {
      for (const decl of node.declarationList.declarations) {
        if (decl.name && ts.isIdentifier(decl.name) && decl.name.text === name) {
          if (decl.initializer &&
              (ts.isArrowFunction(decl.initializer) || ts.isFunctionExpression(decl.initializer))) {
            if (!type || type === 'function' || type === 'component') {
              found = node;
              return;
            }
          }
        }
      }
    }

    // Method inside class
    if (ts.isMethodDeclaration(node)) {
      const methodName = node.name && ts.isIdentifier(node.name) ? node.name.text : null;
      if (methodName === name) {
        if (!type || type === 'method' || type === 'function') {
          found = node;
          return;
        }
      }
    }

    ts.forEachChild(node, visit);
  }

  visit(sourceFile);
  return found;
}

// Main logic
switch (mode) {
  case '--toc': {
    const declarations = collectDeclarations();
    const totalLines = sourceText.split('\n').length;

    console.log(`# ${path.basename(filePath)} (${totalLines} lines)`);
    console.log('');

    if (declarations.length === 0) {
      console.log('(no declarations found)');
      break;
    }

    // Group by type
    const byType = {};
    for (const d of declarations) {
      const key = d.type;
      if (!byType[key]) byType[key] = [];
      byType[key].push(d);
    }

    const typeOrder = ['interface', 'type', 'enum', 'class', 'component', 'function', 'method'];

    for (const type of typeOrder) {
      if (!byType[type] || byType[type].length === 0) continue;

      const typeLabel = {
        'interface': 'Interfaces',
        'type': 'Types',
        'enum': 'Enums',
        'class': 'Classes',
        'component': 'Components',
        'function': 'Functions',
        'method': 'Methods'
      }[type];

      console.log(`## ${typeLabel}`);
      console.log('');

      for (const d of byType[type]) {
        const exportTag = d.exported ? ' [export]' : '';
        const asyncTag = d.async ? ' [async]' : '';
        const parentTag = d.parent ? ` (${d.parent})` : '';
        const lines = d.endLine - d.startLine + 1;
        console.log(`  ${d.name}${parentTag}${exportTag}${asyncTag} - L${d.startLine}-${d.endLine} (${lines} lines)`);
      }
      console.log('');
    }
    break;
  }

  case '--fn':
  case '--function': {
    if (!target) {
      console.error('Error: Function name required');
      process.exit(1);
    }
    // Try function first, then method
    let node = findNodeByName(target, 'function');
    if (!node) node = findNodeByName(target, 'method');
    if (!node) {
      console.error(`Function/method not found: ${target}`);
      process.exit(1);
    }
    console.log(extractWithLineNumbers(node.getFullStart(), node.getEnd()));
    break;
  }

  case '--class': {
    if (!target) {
      console.error('Error: Class name required');
      process.exit(1);
    }
    const node = findNodeByName(target, 'class');
    if (!node) {
      console.error(`Class not found: ${target}`);
      process.exit(1);
    }
    console.log(extractWithLineNumbers(node.getFullStart(), node.getEnd()));
    break;
  }

  case '--interface':
  case '--type': {
    if (!target) {
      console.error('Error: Interface/type name required');
      process.exit(1);
    }
    const node = findNodeByName(target, 'interface');
    if (!node) {
      console.error(`Interface/type not found: ${target}`);
      process.exit(1);
    }
    console.log(extractWithLineNumbers(node.getFullStart(), node.getEnd()));
    break;
  }

  case '--component': {
    if (!target) {
      console.error('Error: Component name required');
      process.exit(1);
    }
    // Components can be functions or classes
    let node = findNodeByName(target, 'component');
    if (!node) node = findNodeByName(target, 'function');
    if (!node) node = findNodeByName(target, 'class');
    if (!node) {
      console.error(`Component not found: ${target}`);
      process.exit(1);
    }
    console.log(extractWithLineNumbers(node.getFullStart(), node.getEnd()));
    break;
  }

  case '--method': {
    if (!target) {
      console.error('Error: Method name required');
      process.exit(1);
    }
    const node = findNodeByName(target, 'method');
    if (!node) {
      console.error(`Method not found: ${target}`);
      process.exit(1);
    }
    console.log(extractWithLineNumbers(node.getFullStart(), node.getEnd()));
    break;
  }

  case '--lines': {
    if (!target) {
      console.error('Error: Line range required (e.g., 100-200)');
      process.exit(1);
    }
    const match = target.match(/^(\d+)-(\d+)$/);
    if (!match) {
      console.error('Error: Invalid line range format. Use: start-end (e.g., 100-200)');
      process.exit(1);
    }
    const startLine = parseInt(match[1], 10);
    const endLine = parseInt(match[2], 10);
    const lines = sourceText.split('\n');
    const totalLines = lines.length;

    if (startLine < 1 || endLine > totalLines || startLine > endLine) {
      console.error(`Error: Invalid line range. File has ${totalLines} lines.`);
      process.exit(1);
    }

    const extracted = lines.slice(startLine - 1, endLine);
    const lineNumWidth = String(endLine).length;
    console.log(extracted.map((line, i) => {
      const lineNum = String(startLine + i).padStart(lineNumWidth);
      return `${lineNum}| ${line}`;
    }).join('\n'));
    break;
  }

  default:
    console.error(`Unknown mode: ${mode}`);
    console.error('Valid modes: --toc, --fn, --class, --interface, --component, --method, --lines');
    process.exit(1);
}
