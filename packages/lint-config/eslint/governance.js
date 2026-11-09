const USER_VISIBLE_ATTRIBUTES = new Set(["alt", "aria-label", "placeholder", "title"]);

function jsxIdentifierName(node) {
  return node?.type === "JSXIdentifier" ? node.name : null;
}

function isDomElement(openingElement) {
  const name = jsxIdentifierName(openingElement.name);
  return name !== null && /^[a-z]/.test(name);
}

function isCssVariablesCall(node) {
  return (
    node.type === "CallExpression" &&
    node.callee.type === "Identifier" &&
    node.callee.name === "cssVariables"
  );
}

function isAllowedStyleExpression(node) {
  if (node.type === "Identifier" && node.name === "undefined") return true;
  if (node.type === "Literal" && node.value === null) return true;
  if (isCssVariablesCall(node)) return true;
  if (node.type === "ConditionalExpression") {
    return isAllowedStyleExpression(node.consequent) && isAllowedStyleExpression(node.alternate);
  }
  if (node.type === "LogicalExpression") {
    return isAllowedStyleExpression(node.right);
  }
  return false;
}

function containsTranslationCall(node) {
  if (!node || typeof node !== "object") return false;
  if (node.type === "CallExpression") {
    const callee = node.callee;
    if (
      (callee.type === "Identifier" && /^t(?:[A-Z].*)?$/.test(callee.name)) ||
      (callee.type === "MemberExpression" &&
        !callee.computed &&
        callee.property.type === "Identifier" &&
        callee.property.name === "t")
    ) {
      return true;
    }
  }
  return Object.entries(node).some(([key, value]) => {
    if (key === "parent") return false;
    if (Array.isArray(value)) return value.some(containsTranslationCall);
    return value !== null && typeof value === "object" && containsTranslationCall(value);
  });
}

function containsVisibleLiteral(node) {
  if (node.type === "Literal") {
    return typeof node.value === "string" && /[A-Za-z\u3400-\u9fff]/.test(node.value);
  }
  if (node.type === "TemplateLiteral") {
    return node.quasis.some((quasi) => /[A-Za-z\u3400-\u9fff]/.test(quasi.value.raw));
  }
  if (node.type === "ConditionalExpression") {
    return containsVisibleLiteral(node.consequent) || containsVisibleLiteral(node.alternate);
  }
  if (node.type === "BinaryExpression") {
    return node.operator === "+" &&
      (containsVisibleLiteral(node.left) || containsVisibleLiteral(node.right));
  }
  if (node.type === "LogicalExpression") {
    if (node.operator === "&&") return containsVisibleLiteral(node.right);
    return containsVisibleLiteral(node.left) || containsVisibleLiteral(node.right);
  }
  if (node.type === "ArrayExpression") {
    return node.elements.some((element) => element !== null && containsVisibleLiteral(element));
  }
  return false;
}

export const governanceRules = {
  "no-dom-inline-style": {
    meta: {
      type: "problem",
      docs: { description: "DOM style only accepts runtime CSS variable injection" },
      schema: [],
      messages: {
        inlineStyle:
          "DOM style 只能注入 CSS 自定义属性；把视觉声明移动到归属 CSS，并使用 cssVariables(...) 传递运行时值。",
      },
    },
    create(context) {
      return {
        JSXAttribute(node) {
          if (jsxIdentifierName(node.name) !== "style" || !isDomElement(node.parent)) return;
          const expression = node.value?.type === "JSXExpressionContainer" ? node.value.expression : null;
          if (expression === null || expression.type === "JSXEmptyExpression") return;
          if (!isAllowedStyleExpression(expression)) context.report({ node, messageId: "inlineStyle" });
        },
      };
    },
  },
  "no-user-visible-literal": {
    meta: {
      type: "problem",
      docs: { description: "Rendered UI copy must come from i18n or a typed product fact" },
      schema: [],
      messages: {
        literal: "用户可见文案必须来自 i18n 或 typed 产品事实，不能直接写入 JSX。",
      },
    },
    create(context) {
      return {
        JSXText(node) {
          if (/[A-Za-z\u3400-\u9fff]/.test(node.value)) context.report({ node, messageId: "literal" });
        },
        JSXExpressionContainer(node) {
          if (node.parent.type === "JSXAttribute" || node.expression.type === "JSXEmptyExpression") {
            return;
          }
          if (containsVisibleLiteral(node.expression)) context.report({ node, messageId: "literal" });
        },
        JSXAttribute(node) {
          if (!USER_VISIBLE_ATTRIBUTES.has(jsxIdentifierName(node.name))) return;
          const value = node.value;
          if (
            (value?.type === "Literal" && containsVisibleLiteral(value)) ||
            (value?.type === "JSXExpressionContainer" &&
              value.expression.type !== "JSXEmptyExpression" &&
              containsVisibleLiteral(value.expression))
          ) {
            context.report({ node, messageId: "literal" });
          }
        },
      };
    },
  },
  "no-translation-concatenation": {
    meta: {
      type: "problem",
      docs: { description: "Translated copy must use interpolation instead of string concatenation" },
      schema: [],
      messages: {
        concatenation: "译文不能用 + 拼接；请改为单个 i18n key 的插值或复数规则。",
      },
    },
    create(context) {
      return {
        BinaryExpression(node) {
          if (node.operator === "+" && containsTranslationCall(node)) {
            context.report({ node, messageId: "concatenation" });
          }
        },
        TemplateLiteral(node) {
          if (node.expressions.some(containsTranslationCall)) {
            context.report({ node, messageId: "concatenation" });
          }
        },
      };
    },
  },
};
