# Растровые эффекты из PhotoCraft

VectorCraft использует фильтры соседнего приложения **PhotoCraft** (крейт `photocraft-algo`, те же авторы,
MIT OR Apache-2.0) как живые растровые эффекты оформления. Они применяются к любому объекту, у которого
работает растровая тень: контурам и составным контурам, группам и слоям (к их общему изображению), тексту,
встроенным изображениям, экземплярам символов, живым объектам (переходы, оболочки, сетки), а также к
отдельной заливке или обводке.

## Откуда берётся код

Крейты `photocraft-algo`, `photocraft-raster`, `photocraft-geom`, `photocraft-color` (и их зависимость
`photocraft-cms`) подключены в корневом `Cargo.toml` как **git-зависимости, закреплённые на коммите**
`44bf13e8903316d17291216adadc15fc357cb875` репозитория <https://github.com/storytold/photocraft>.
Происхождение и лицензия записаны в `NOTICE`. В вебе (wasm32) фильтры работают однопоточно: PhotoCraft
сам подключает `rayon` только для не-wasm целей, поэтому никаких feature-флагов не нужно.

## Идентификаторы эффектов

Эффект — обычная запись `{id, params}` в стеке оформления (`effect.apply`, `effect.setParams`,
`effect.list`). Полный список с параметрами и значениями по умолчанию отдаёт `effect.list`.

| Меню | id |
|---|---|
| Effect › Blur | `blur.box`, `blur.motion`, `blur.radial`, `blur.smart`, `blur.surface`, `blur.lens`, `blur.shape`, `blur.tiltShift`, `blur.iris`, `blur.spin` (и прежний собственный `blur.gaussian`) |
| Raster Effects › Sharpen | `sharpen.unsharpMask`, `sharpen.smart` |
| Raster Effects › Noise | `noise.add`, `noise.median`, `noise.dustAndScratches`, `noise.reduce` |
| Raster Effects › Pixelate | `pixelate.colorHalftone`, `pixelate.crystallize`, `pixelate.facet`, `pixelate.fragment`, `pixelate.mezzotint`, `pixelate.mosaic`, `pixelate.pointillize` |
| Raster Effects › Render | `render.lensFlare`, `render.lightingEffects` |
| Raster Effects › Stylize | `stylize.diffuse`, `stylize.emboss`, `stylize.extrude`, `stylize.findEdges`, `stylize.oilPaint`, `stylize.solarize`, `stylize.tiles`, `stylize.traceContour`, `stylize.wind`, `gallery.glowingEdges` |
| Raster Effects › Distort | `distort.twirl`, `distort.pinch`, `distort.spherize`, `distort.wave`, `distort.ripple`, `distort.polarCoordinates`, `distort.shear`, `distort.rasterZigZag` (не путать с контурным `distort.zigZag`), `gallery.diffuseGlow`, `gallery.glass`, `gallery.oceanRipple` |
| Raster Effects › Other | `other.highPass`, `other.minimum`, `other.maximum`, `other.offset`, `other.custom`, `other.hsbHsl`, `other.invert`, `other.desaturate` |
| Raster Effects › Video | `video.deInterlace`, `video.ntscColors` |
| Raster Effects › Artistic, Brush Strokes, Sketch, Texture | `gallery.<ключ>` — все 47 фильтров галереи: `gallery.coloredPencil`, `gallery.cutout`, `gallery.watercolor`, `gallery.accentedEdges`, `gallery.chalkCharcoal`, `gallery.craquelure`, `gallery.texturizer`… |

Пример: `{"command":"effect.apply","params":{"effect":"blur.motion","params":{"angle":30,"distance":12}}}`.

## Как это работает

1. `vectorcraft-effects` (`crates/effects/src/pixel.rs`) описывает каждый эффект: пункт меню, документацию
   параметров, значения по умолчанию и какие параметры — расстояния в пунктах. Параметры проверяются:
   не-числа, бесконечности и неизвестные варианты — ошибка команды (`effect.apply`/`effect.setParams`
   отвечают ошибкой, документ не меняется); числа зажимаются в диапазоны диалогов.
2. Рендерер (`crates/render/src/pixelfx.rs`) рисует объект (вместе с растровыми эффектами, стоящими в стеке
   до фильтра) во внеэкранный растр, отдаёт его фильтру и рисует результат на место объекта. Эффекты после
   фильтра применяются к уже отфильтрованным пикселям (тень после «Скручивания» — тень скрученного объекта).
3. Мост `pixel::run_filter`: премультиплицированный sRGB RGBA8 рендерера → прямая (straight) альфа во
   float-поверхности PhotoCraft (`RGBA32F`) → `photocraft_algo::apply` → обратно в премультиплицированный
   RGBA8. Полностью прозрачные пиксели остаются прозрачными; NaN и выход за 0…1 зажимаются.
4. **Поля (halo).** Растр покрывает объект плюс радиус чтения/растекания фильтра (`FilterParams::halo`),
   поэтому размытие у края не обрезается; этот же запас входит в границы отсечения и в `effects::outset`.
   Фильтры-соседства берут только видимую часть экрана (плюс запас), «глобальные» искажения (twirl, wave…)
   — весь объект, центрируясь на его границах.
5. **Разрешение.** Расстояния (`radius`, `distance`, `cellSize`, `height`…) заданы в пунктах и переводятся в
   пиксели по текущему масштабу, поэтому вид не зависит от зума и разрешения экспорта. Фильтры, чей вид
   привязан к пиксельной сетке (шум, фильтры галереи, Facet, Find Edges…), считаются при разрешении
   Document Raster Effects Settings и масштабируются к виду. Слишком большие расстояния и растры больше
   8 Мпикс понижают рабочее разрешение вместо обрезки параметров.
6. **Кэш.** Отфильтрованный растр целого объекта переиспользуется, пока объект, эффекты и масштаб/поворот
   вида не меняются: панорамирование не запускает фильтр заново.
7. **Экспорт.** PDF (и Expand Appearance) превращают растровые эффекты в изображения при разрешении
   документа, как и раньше. SVG рисует тени и свечения SVG-фильтрами, а объекты с фильтрами PhotoCraft
   заменяются встроенными PNG (`flatten_raster_filters`), в том числе при копировании в буфер как SVG.

## Чего пока нет

- **Effect Gallery** как отдельный диалог со стопкой фильтров и превью-миниатюрами: каждый фильтр галереи
  доступен по отдельности с общим диалогом параметров.
- **Field Blur** и **Path Blur** (нужны интерактивные булавки/контуры на холсте), **Displace** (нужна карта
  смещения из файла), **Fibers** и прочие генераторы, не зависящие от изображения.
- В Lighting Effects — один источник света; Lens Blur — без карты глубины.
- Вид фильтров галереи — приближения PhotoCraft (clean-room), а не точная копия Photoshop.
