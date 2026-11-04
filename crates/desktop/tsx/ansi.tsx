import { Text, View, token } from "nsh";

function color(name: string): string {
  return token(name as never) as string;
}

interface Rgb {
  r: number;
  g: number;
  b: number;
}

interface Style {
  fg?: Rgb;
  bg?: Rgb;
  bold?: boolean;
  dim?: boolean;
  italic?: boolean;
  underline?: boolean;
}

export interface Span {
  text: string;
  style: Style;
}

export interface Line {
  spans: Span[];
}

const STEPS = [0, 0x5f, 0x87, 0xaf, 0xd7, 0xff];

function palette(index: number): Rgb | undefined {
  const standard: Rgb[] = [
    { r: 0x1c, g: 0x1c, b: 0x1e },
    { r: 0xc7, g: 0x3e, b: 0x3e },
    { r: 0x2f, g: 0x8f, b: 0x4f },
    { r: 0xb8, g: 0x86, b: 0x0b },
    { r: 0x2b, g: 0x6c, b: 0xd9 },
    { r: 0x9a, g: 0x4b, b: 0xc4 },
    { r: 0x1d, g: 0x8f, b: 0x9a },
    { r: 0xd8, g: 0xd8, b: 0xdc },
    { r: 0x6e, g: 0x6e, b: 0x73 },
    { r: 0xff, g: 0x6b, b: 0x6b },
    { r: 0x4f, g: 0xd1, b: 0x7a },
    { r: 0xf2, g: 0xc0, b: 0x4c },
    { r: 0x6c, g: 0xa8, b: 0xff },
    { r: 0xd2, g: 0x8b, b: 0xff },
    { r: 0x4f, g: 0xd6, b: 0xe0 },
    { r: 0xff, g: 0xff, b: 0xff },
  ];
  if (index < 16) return standard[index];
  if (index < 232) {
    const value = index - 16;
    return { r: STEPS[Math.floor(value / 36)], g: STEPS[Math.floor((value % 36) / 6)], b: STEPS[value % 6] };
  }
  const level = 8 + (index - 232) * 10;
  return { r: level, g: level, b: level };
}

function hex(color: Rgb): string {
  const value = (color.r << 16) | (color.g << 8) | color.b;
  return "#" + value.toString(16).padStart(6, "0");
}

function apply(style: Style, params: number[]): Style {
  const next: Style = { ...style };
  if (params.length === 0) return {};
  for (let index = 0; index < params.length; index += 1) {
    const param = params[index];
    if (param === 0) {
      return {};
    } else if (param === 1) next.bold = true;
    else if (param === 2) next.dim = true;
    else if (param === 3) next.italic = true;
    else if (param === 4) next.underline = true;
    else if (param === 22) {
      next.bold = false;
      next.dim = false;
    } else if (param === 23) next.italic = false;
    else if (param === 24) next.underline = false;
    else if (param >= 30 && param <= 37) next.fg = palette(param - 30);
    else if (param === 39) next.fg = undefined;
    else if (param >= 40 && param <= 47) next.bg = palette(param - 40);
    else if (param === 49) next.bg = undefined;
    else if (param >= 90 && param <= 97) next.fg = palette(param - 90 + 8);
    else if (param >= 100 && param <= 107) next.bg = palette(param - 100 + 8);
    else if (param === 38 || param === 48) {
      const foreground = param === 38;
      const mode = params[index + 1];
      if (mode === 5 && params[index + 2] != null) {
        const color = palette(params[index + 2]);
        if (foreground) next.fg = color;
        else next.bg = color;
        index += 2;
      } else if (mode === 2 && params[index + 4] != null) {
        const color = { r: params[index + 2], g: params[index + 3], b: params[index + 4] };
        if (foreground) next.fg = color;
        else next.bg = color;
        index += 4;
      }
    }
  }
  return next;
}

export function parseAnsi(input: string): Line[] {
  const lines: Line[] = [];
  let line: Line = { spans: [] };
  let style: Style = {};
  let text = "";
  const push = () => {
    if (!text) return;
    const last = line.spans[line.spans.length - 1];
    if (last && sameStyle(last.style, style)) last.text += text;
    else line.spans.push({ text, style });
    text = "";
  };
  for (let index = 0; index < input.length; index += 1) {
    const char = input[index];
    if (char === "\u001b") {
      push();
      const next = input[index + 1];
      if (next === "[") {
        index += 2;
        let params = "";
        while (index < input.length && !(input[index] >= "@" && input[index] <= "~")) {
          params += input[index];
          index += 1;
        }
        const final = input[index];
        if (final === "m") {
          const values = params.split(";").map((part) => (part.trim() ? Number(part) : 0));
          style = apply(style, values);
        }
      } else if (next === "]") {
        index += 2;
        while (index < input.length) {
          if (input[index] === "\u0007") break;
          if (input[index] === "\u001b" && input[index + 1] === "\\") {
            index += 1;
            break;
          }
          index += 1;
        }
      } else {
        index += 1;
      }
      continue;
    }
    if (char === "\n") {
      push();
      lines.push(line);
      line = { spans: [] };
      continue;
    }
    if (char === "\r") {
      text = "";
      line = { spans: [] };
      continue;
    }
    if (char === "\t") {
      text += "    ";
      continue;
    }
    text += char;
  }
  push();
  if (line.spans.length > 0 || lines.length > 0) lines.push(line);
  return lines;
}

function sameStyle(a: Style, b: Style): boolean {
  return (
    a.fg === b.fg &&
    a.bg === b.bg &&
    a.bold === b.bold &&
    a.dim === b.dim &&
    a.italic === b.italic &&
    a.underline === b.underline
  );
}

export function AnsiBlock(props: { text: string }) {
  const lines = parseAnsi(props.text);
  return (
    <View
      style={{
        flexDirection: "column",
        gap: 1,
        padding: 10,
        borderRadius: 8,
        borderWidth: 1,
        borderColor: color("border"),
        backgroundColor: color("background"),
      }}
    >
      {lines.map((line, lineIndex) => (
        <View style={{ flexDirection: "row", flexWrap: "wrap", minHeight: 16 }}>
          {line.spans.map((span) => (
            <Text
              style={{
                fontFamily: "Lilex",
                fontSize: 11.5,
                color: span.style.fg ? hex(span.style.fg) : color("foreground"),
                backgroundColor: span.style.bg ? hex(span.style.bg) : "transparent",
                fontWeight: span.style.bold ? 600 : 400,
                fontStyle: span.style.italic ? "italic" : "normal",
                textDecorationLine: span.style.underline ? "underline" : "none",
                opacity: span.style.dim ? 0.65 : 1,
              }}
            >
              {span.text.replaceAll(" ", " ")}
            </Text>
          ))}
        </View>
      ))}
    </View>
  );
}
