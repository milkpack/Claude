# Процедурная генерация (ядро)

Процедурный объект — это группа, дочерние объекты которой **генерируются** из графа узлов
(в духе [Graphite](https://graphite.art)). Граф хранится в поле `Node.procedural`
(`vectorcraft_doc::ProcGraph`); вычисляет его крейт `vectorcraft-procedural` (слой L2), а
команды движка `procedural.*` пересобирают детей после каждой правки — в том же шаге отмены.
Сгенерированные дети — обычные узлы документа, поэтому рендер, экспорт SVG/PDF, хит-тест,
копирование/вставка и совместное редактирование работают без изменений.

UI (панель графа узлов) ещё не сделан: его будут строить поверх команд ниже.

## Понятия

- **Граф** `ProcGraph { version, nodes, output?, seed, transform }`.
  - `nodes: [ProcNode]`, `ProcNode { id: u32, kind, params: {имя: значение}, inputs: [Link|null], position: [x, y], name?, bypass?, art? }`.
  - `Link { node: u32, port?: u32 }` — провод с выхода `port` (в v1 всегда 0) узла `node`;
    `inputs[i]` питает входной порт `i`.
  - `output` — узел, чей результат становится содержимым группы (нет — первый узел `output`).
  - `seed` — зерно всех случайных узлов (каждый узел ещё подмешивает свой `id` и параметр `seed`).
  - `transform` — графическое пространство → документ. Перемещение/поворот/масштаб объекта
    накапливаются здесь, поэтому после пересборки арт остаётся там, где его оставил пользователь.
- **Провода несут геометрию**: список элементов (item) — путь, точка или копия пользовательского
  арта; у каждого стиль (заливка, обводка, толщина, непрозрачность), трансформация и атрибуты
  `index`, `random`. Параметры — простые значения в узле (входы-значения можно добавить позже:
  в каталоге у портов есть `type`, сейчас всегда `geometry`).
- **Ошибки по узлам**: неизвестный тип, плохой параметр, цикл → ошибка этого узла, его выход пуст,
  остальной граф считается. Предупреждения — обрезка по лимитам, битые ссылки и т. п.
- **Детерминизм**: тот же граф и seed дают побитно тот же результат на любой машине и в wasm
  (свой SplitMix64, свой градиентный шум, тригонометрия из `libm`, без порядка HashMap в выходе).
  Исключение: узлы на `vectorcraft-pathops` (boolean, offset, simplify, smooth) детерминированы в
  пределах сборки, побитная кроссплатформенность там не гарантирована.
- **Лимиты** (`vectorcraft_procedural::limits`): ≤ 20 000 элементов на выход узла, ≤ 100 000
  узлов документа, ≤ 400 000 опорных точек, ≤ 20 000 точек в пути, ≤ 256 узлов в графе,
  boolean/offset/simplify — ≤ 10 000 точек на входе. Параметры зажимаются в диапазон каталога.
- **Кэш**: результаты узлов кэшируются по хэшу всего, от чего они зависят; при перетаскивании
  слайдера пересчитываются только узлы ниже изменённого.

## Каталог узлов

Полный машиночитаемый каталог (порты, параметры, типы, умолчания, min/max, варианты) отдаёт
`procedural.catalogue`. Углы — в градусах, против часовой стрелки на странице.

| Категория | Узел | Что делает |
|---|---|---|
| generate | `generate.rectangle` | прямоугольник (width, height, radius) с центром в начале координат |
| | `generate.ellipse` | эллипс (width, height) |
| | `generate.polygon` | правильный многоугольник (sides, radius) |
| | `generate.star` | звезда (points, radius, innerRatio) |
| | `generate.line` | отрезок (x1, y1, x2, y2) |
| | `generate.spiral` | спираль (turns, innerRadius, outerRadius, growth linear/exponential, clockwise) |
| | `generate.arc` | дуга (radius, startAngle, sweep, closure open/chord/pie) |
| source | `source.art` | встроенные копии объектов документа (separate: по объекту на элемент) |
| points | `points.scatter` | случайные точки в прямоугольнике или внутри входных форм (count, seed) |
| | `points.poisson` | Poisson-disk (distance, maxCount), в прямоугольнике или внутри форм |
| | `points.grid` | сетка точек (columns, rows, spacingX/Y, stagger) |
| | `points.circle` | точки по окружности, повёрнутые по касательной |
| | `points.alongPath` | точки вдоль путей (count или spacing), повёрнутые по касательной |
| | `points.anchors` | точка на каждой опорной точке |
| instance | `instance.linear` | копии со сдвигом/поворотом/масштабом на шаг (pivot center/origin) |
| | `instance.grid` | копии сеткой |
| | `instance.radial` | копии по кругу (count, radius, startAngle, sweep, rotate) |
| | `instance.copyToPoints` | копия входа «instance» на каждую точку входа «points» (align, scale) |
| | `instance.mirror` | отражение (vertical/horizontal/both, offset, keepOriginal) |
| modify | `modify.transform` | сдвиг, поворот, масштаб, скос (pivot, each) |
| | `modify.jitter` | случайный сдвиг/поворот/масштаб каждого элемента |
| | `modify.noise` | подразбиение и смещение фрактальным шумом (amplitude, frequency, octaves, detail, smooth) |
| | `modify.roundCorners` | скругление углов |
| | `modify.smooth` | сглаживание |
| | `modify.offset` | Offset Path |
| | `modify.simplify` | упрощение |
| | `modify.morph` | промежуточные формы между входами A и B (t) |
| | `modify.boolean` | union/subtract/intersect/exclude/divide (A с B, или элементы A между собой) |
| | `modify.merge` | объединение любого числа входов |
| | `modify.select` | отбор: каждый n-й, диапазон, случайная доля, invert |
| | `modify.order` | reverse, shuffle, сортировка по x/y/size/radius |
| | `modify.boundingBox` | рамки вокруг всех или каждого элемента |
| style | `style.fill`, `style.stroke` | цвет заливки / цвет и толщина обводки |
| | `style.colorRamp` | градиент цветов по элементам (by order/index/random/x/y/radius, rgb/hsl) |
| | `style.hueShift` | сдвиг тона, нарастающий по элементам |
| | `style.opacityRamp` | градиент непрозрачности |
| | `style.randomColor` | случайный цвет из палитры |
| output | `output` | результат графа |

Любой узел можно «обойти» (`bypass: true`): он пропускает свой первый вход без изменений.

## Команды

Все команды, кроме запросов, записываются в журнал и отменяются одним шагом. `id` — объект;
без него берётся выделенный процедурный объект (или тот, внутри которого выделение).
Ответы правок содержат `errors: [{node?, level: "error"|"warning", message}]`.

| Команда | Параметры → результат |
|---|---|
| `procedural.create` (Object › Procedural › Make) | `{preset? \| graph?, x?, y?, seed?, name?}` → `{id, errors}`; без preset/graph выделение становится узлом `source.art` (на том же месте), без выделения — пресет `flower` |
| `procedural.presets` | `{}` → `[{id, label, doc}]` |
| `procedural.catalogue` | `{}` → `{version, categories, kinds: [...]}` |
| `procedural.get` | `{id?}` → `{id, graph, errors, items}` |
| `procedural.errors` | `{id?}` → `{errors}` |
| `procedural.setGraph` | `{id?, graph}` → `{id, errors}` (transform сохраняется, если не задан) |
| `procedural.addNode` | `{id?, kind, params?, position?, name?}` → `{node, errors}` |
| `procedural.removeNode` | `{id?, node}` → `{errors}` (провода к узлу снимаются) |
| `procedural.connect` | `{id?, from, to, port?}` → `{errors}`; циклы и лишние порты — ошибка |
| `procedural.disconnect` | `{id?, to, port?}` → `{errors}` |
| `procedural.setParam` | `{id?, node, param, value}` или `{id?, node, params: {...}}`; `null` — сброс к умолчанию |
| `procedural.setNode` | `{id?, node, position?, name?, bypass?}` |
| `procedural.setOutput` | `{id?, node \| null}` |
| `procedural.setSeed` | `{id?, seed}` → `{seed, errors}` |
| `procedural.reseed` (меню) | `{id?}` → `{seed, errors}` (новый seed выводится из старого — повтор журнала даёт тот же арт) |
| `procedural.expand` (меню) | `{id?}` → `{id}`: граф удаляется, остаётся обычная группа |

**Слайдеры.** Перетаскивание — это живое взаимодействие: `Session::begin_interaction`,
`Session::preview("procedural.setParam", …)` на каждый шаг, `Session::commit_interaction` в конце
(в UI это `panels::live_run`). Всё перетаскивание — один шаг отмены.

Пример:

```json
{"command": "procedural.create", "params": {"preset": "dots", "x": 300, "y": 200}}
{"command": "procedural.addNode", "params": {"kind": "modify.noise", "params": {"amplitude": 8}}}
{"command": "procedural.connect", "params": {"from": 3, "to": 7}}
{"command": "procedural.setParam", "params": {"node": 1, "param": "count", "value": 400}}
```

## Пресеты

`flower` (радиальный цветок), `dots` (рассыпанные точки с цветовым градиентом), `noiseWaves`
(линии, смещённые шумом), `jitterSquares` (сетка повёрнутых квадратов), `spiralCircles`
(спираль из кругов), `starburst` (лучи вокруг звезды), `confetti` (Poisson-конфетти),
`booleanPattern` (пластина с отверстиями — boolean subtract), `hexTiles` (соты).

## Известные ограничения

- Панели графа узлов пока нет (только команды).
- Правка отдельных сгенерированных детей теряется при следующей пересборке (как у графиков).
- При масштабировании объекта с «Scale Strokes» толщина обводок после пересборки возвращается к
  значению из графа.
- Offset/boolean на сильно самоперекрывающейся геометрии может занимать секунды (лимиты по числу
  точек это ограничивают, но не исключают).
