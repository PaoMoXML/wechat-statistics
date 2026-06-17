//! HTML 渲染：把情侣报告渲染成自包含单页（内联 CSS、无外部依赖、离线可看）。
//!
//! 词云用「按频率加权字体大小」的纯文字布局，避免引入 JS 词云库；
//! 全部数据内联进 HTML，双击即可在浏览器打开，不联网。

use crate::couple::CoupleReport;
use crate::model::Conversation;

/// 生成完整 HTML 文档。
pub fn couple_html(conv: &Conversation, name: &str, r: &CoupleReport, c: &crate::curios::Curios) -> String {
    let css = format!("{}\n{}", BASE_CSS, DASHBOARD_CSS);
    let _ = conv; // 预留：将来可展示 username
    let total = r.total;
    let my_pct = crate::fmt::pct(r.my_count, total);
    let other_pct = 100.0 - my_pct;

    let span = match (&r.first_day, &r.last_day) {
        (Some(a), Some(b)) => format!("{} ~ {}", esc(a), esc(b)),
        _ => String::new(),
    };

    HTML_TEMPLATE
        .replace("{{CSS}}", &css)
        .replace("{{TITLE}}", &format!("💑 {} 的情侣报告", esc(name)))
        .replace("{{NAME}}", &esc(name))
        .replace("{{SPAN}}", &span)
        .replace("{{NTH}}", &fmt(r.nth_day_today))
        .replace("{{TOTAL}}", &fmt(total))
        .replace("{{CARDS}}", &cards_html(r))
        .replace(
            "{{RATIO}}",
            &format!(
                "<div class=\"bar\"><span class=\"me\" style=\"width:{a:.1}%\"></span><span class=\"them\" style=\"width:{b:.1}%\"></span></div><div class=\"bar-legend\"><span>我 {c}</span><span>ta {d}</span></div>",
                a = my_pct, b = other_pct, c = fmt(r.my_count), d = fmt(r.other_count),
            ),
        )
        .replace("{{REPLY}}", &reply_html(r))
        .replace("{{CLOUD}}", &cloud_html(r))
        .replace("{{KEYWORDS}}", &keywords_html(r))
        .replace("{{TOPWORDS}}", &topwords_html(r))
        .replace("{{DIG}}", &dig_html(r))
        .replace("{{TREND}}", &trend_section(r.trend.as_ref(), r.trend_monthly.as_ref()))
        .replace("{{CURIOS}}", &curios_html(c))
}

/// 生成 PPT 风格的全屏翻页 HTML（键盘/滚动/点击/滑动切换）。
pub fn couple_slides(conv: &Conversation, name: &str, r: &CoupleReport, c: &crate::curios::Curios) -> String {
    let _ = conv;
    let slides = build_slides(name, r, c);
    let n = slides.len();
    let dots: String = (0..n).map(|_| "<i></i>").collect::<Vec<_>>().join("");
    let body: String = slides.iter().map(|s| format!("<div class=\"slide\">{s}</div>")).collect::<Vec<_>>().join("");
    let css = format!("{}\n{}", BASE_CSS, DECK_CSS);
    SLIDES_SHELL
        .replace("{{CSS}}", &css)
        .replace("{{TITLE}}", &format!("💑 {} · 情侣报告", esc(name)))
        .replace("{{SLIDES}}", &body)
        .replace("{{DOTS}}", &dots)
}

fn build_slides(name: &str, r: &CoupleReport, c: &crate::curios::Curios) -> Vec<String> {
    let first = r.first_day.as_deref().unwrap_or("—");
    let last = r.last_day.as_deref().unwrap_or("—");
    let mut s: Vec<String> = Vec::new();

    // 1. 封面
    s.push(format!(
        "<div class=\"s-cover\"><div class=\"emoji\">💑</div><h1>{name} 的情侣报告</h1><p class=\"sub\">{first} ~ {last} · 第 {nth} 天</p><p class=\"big\">{total} 条消息</p><p class=\"hint\">← → 方向键 · 滚动 · 点击两侧 · 滑动 翻页</p></div>",
        name = esc(name), nth = fmt(r.nth_day_today), total = fmt(r.total),
    ));

    // 2. 关系温度
    let bars: String = c
        .score
        .breakdown
        .iter()
        .map(|(k, p, m)| {
            let pc = if *m > 0 { *p as f64 / *m as f64 * 100.0 } else { 0.0 };
            format!("<div class=\"sb\"><span>{}</span><div class=\"sbb\"><i style=\"width:{}%\"></i></div><u>{}/{}</u></div>", esc(k), pc as i64, p, m)
        })
        .collect();
    s.push(format!(
        "<h2>🌡️ 关系温度</h2><div class=\"s-score\">{score}<span>/100</span></div><div class=\"s-scorelabel\">{label}</div><div class=\"score-bars\" style=\"max-width:560px;margin:0 auto\">{bars}</div>",
        score = c.score.score, label = c.score.label,
    ));

    // 3. 时间线
    let sil_note = match &r.longest_silence_range {
        Some((a, b)) => format!("<div class=\"s-note\">最长沉默 {n} 天（{a} → {b}）</div>", n = fmt(r.longest_silence_days)),
        None => String::new(),
    };
    s.push(format!(
        "<h2>📅 我们的时间线</h2><div class=\"s-stats\"><div class=\"s-stat\"><b>{nth}</b><span>在一起第几天</span></div><div class=\"s-stat\"><b>{a}/{nth}</b><span>有聊天的天</span></div><div class=\"s-stat\"><b>{st}</b><span>最长连续(天)</span></div><div class=\"s-stat\"><b>{si}</b><span>最长沉默(天)</span></div></div>{sil_note}",
        nth = fmt(r.nth_day_today), a = fmt(r.active_days), st = fmt(r.longest_streak), si = fmt(r.longest_silence_days),
    ));

    // 4. 高光时刻
    let (pkd, pkn) = r.peak_day.as_ref().map(|(d, n)| (esc(d), *n)).unwrap_or_else(|| ("—".into(), 0));
    s.push(format!(
        "<h2>✨ 高光时刻</h2><div class=\"s-stats\"><div class=\"s-stat\"><b>{pkn}</b><span>最嗨一天 {pkd}</span></div><div class=\"s-stat\"><b>{late}</b><span>一起熬夜(条)</span></div><div class=\"s-stat\"><b>{calls}</b><span>通话次数</span></div><div class=\"s-stat\"><b>{rally}</b><span>最长对线(条)</span></div></div>",
        late = fmt(r.late_night_count), calls = fmt(r.call_count), rally = fmt(c.rally.max_len),
    ));

    // 5. 你来我往
    let my_pct = crate::fmt::pct(r.my_count, r.total);
    let ratio = format!(
        "<div class=\"bar\"><span class=\"me\" style=\"width:{a:.1}%\"></span><span class=\"them\" style=\"width:{b:.1}%\"></span></div><div class=\"bar-legend\"><span>我 {c}</span><span>ta {d}</span></div>",
        a = my_pct, b = 100.0 - my_pct, c = fmt(r.my_count), d = fmt(r.other_count),
    );
    let reply = format!(
        "{}{}",
        bar_row("秒回率（30s）", r.my_quick_reply_ratio, r.other_quick_reply_ratio),
        bar_row("1 分钟内", r.my_fast_reply_ratio, r.other_fast_reply_ratio),
    );
    s.push(format!(
        "<h2>📊 你来我往</h2><div style=\"max-width:600px;margin:0 auto\">{ratio}</div><div style=\"max-width:560px;margin:28px auto 0\">{reply}</div>",
    ));

    // 6. 第一次
    let firsts: String = c
        .firsts
        .iter()
        .filter_map(|f| {
            f.when.as_ref().map(|w| {
                let snip = f.snippet.as_deref().unwrap_or("");
                let tail = if snip.is_empty() { String::new() } else { format!("「{}」", esc(snip)) };
                format!("<div class=\"mi\"><span class=\"ml\">{}</span><span class=\"md\">{} {} {}</span></div>", esc(&f.label), esc(w), f.who.unwrap_or(""), tail)
            })
        })
        .collect();
    s.push(format!("<h2>🏁 第一次纪念日</h2><section style=\"max-width:660px\"><div class=\"milestones\">{firsts}</div></section>"));

    // 7. 趋势
    s.push(format!("<h2>📈 热度趋势</h2>{}", trend_section(r.trend.as_ref(), r.trend_monthly.as_ref())));

    // 8. 生物钟
    s.push(format!("<h2>🕐 聊天生物钟</h2><section style=\"max-width:780px\">{}</section>", biorhythm_svg(&c.biorhythm)));

    // 9. 热力图
    s.push(format!("<h2>🗓️ 全年热力图</h2>{}", heatmap_html(&c.heatmap)));

    // 10. 词云
    s.push(format!("<h2>☁️ 高频词云</h2><section style=\"max-width:780px\">{}</section>", cloud_html(r)));

    // 11. 口癖
    s.push(format!(
        "<h2>🗣️ 各自口癖</h2><div class=\"sign\" style=\"max-width:680px;margin:0 auto\"><div><span class=\"lbl me\">你的口癖</span><div class=\"chips\">{mine}</div></div><div><span class=\"lbl them\">ta 的口癖</span><div class=\"chips\">{theirs}</div></div></div>",
        mine = sig_chips(&c.signature.mine), theirs = sig_chips(&c.signature.theirs),
    ));

    // 12. 关键词
    s.push(format!("<h2>💬 关键词</h2>{}", keywords_html(r)));

    // 13. 深挖
    s.push(format!("<h2>🔎 单会话深挖</h2>{}", dig_html(r)));

    // 14. 结尾
    let chars = r.text.as_ref().map(|t| fmt(t.my_chars + t.other_chars)).unwrap_or_else(|| "—".into());
    s.push(format!(
        "<div class=\"emoji\">💌</div><h1 style=\"font-size:38px;margin:10px 0\">谢谢观看</h1><p class=\"sub\">{total} 条消息 · {chars} 字 · 最长连续 {st} 天 · 温度 {score}/100</p><p class=\"hint\">由 wechat-statistics 本地生成 · 全程离线</p>",
        total = fmt(r.total), st = fmt(r.longest_streak), score = c.score.score,
    ));

    s
}

fn cards_html(r: &CoupleReport) -> String {
    let streak_sub = if r.current_streak > 0 {
        format!("<div class=\"card-sub\">当前已连续 {} 天</div>", fmt(r.current_streak))
    } else {
        "<div class=\"card-sub\">当前已中断</div>".to_string()
    };
    let silence_sub = match &r.longest_silence_range {
        Some((a, b)) => format!("<div class=\"card-sub\">{} → {}</div>", esc(a), esc(b)),
        None => String::new(),
    };
    let (peak_val, peak_sub) = match &r.peak_day {
        Some((d, n)) => (esc(d), format!("<div class=\"card-sub\">{} 条</div>", fmt(*n))),
        None => (String::new(), String::new()),
    };
    let active_sub = format!("<div class=\"card-sub\">{:.0}% 有交流</div>", r.active_ratio * 100.0);

    let cards = [
        card("🔥 最长连续", &fmt(r.longest_streak), &streak_sub),
        card("🤫 最长沉默", &format!("{} 天", fmt(r.longest_silence_days)), &silence_sub),
        card("🎉 最嗨一天", &peak_val, &peak_sub),
        card("🌙 一起熬夜", &format!("{} 条", fmt(r.late_night_count)), ""),
        card("📞 通话", &format!("{} 次", fmt(r.call_count)), ""),
        card("📅 活跃", &format!("{} / {}", fmt(r.active_days), fmt(r.nth_day_today)), &active_sub),
    ];
    format!("<div class=\"cards\">{}</div>", cards.join(""))
}

fn card(label: &str, value: &str, sub: &str) -> String {
    format!(
        "<div class=\"card\"><div class=\"card-val\">{}</div><div class=\"card-label\">{}</div>{}</div>",
        value, label, sub,
    )
}

fn reply_html(r: &CoupleReport) -> String {
    if r.my_reply_count + r.other_reply_count == 0 {
        return "<p class='muted'>无回复样本</p>".into();
    }
    format!("{}{}", bar_row("秒回率（30s）", r.my_quick_reply_ratio, r.other_quick_reply_ratio),
            bar_row("1 分钟内", r.my_fast_reply_ratio, r.other_fast_reply_ratio))
}

fn bar_row(label: &str, me: f64, other: f64) -> String {
    // 入参是 0.0–1.0 的比率，这里换算成百分比。
    format!(
        "<div class=\"reply-row\"><div class=\"reply-label\">{}</div><div class=\"reply-bars\">{}</div></div>",
        esc(label),
        bar_line("me", "我", me * 100.0) + &bar_line("them", "ta", other * 100.0),
    )
}

fn bar_line(cls: &str, who: &str, pct: f64) -> String {
    format!(
        "<div class=\"rbi\"><span class=\"who {cls}\">{who}</span><div class=\"rb\"><span class=\"{cls}\" style=\"width:{pct:.1}%\"></span></div><i>{pct:.1}%</i></div>",
    )
}

fn cloud_html(r: &CoupleReport) -> String {
    let Some(wf) = &r.words else {
        return "<p class='muted'>未计算高频词（加 --words N）</p>".into();
    };
    if wf.overall.is_empty() {
        return "<p class='muted'>无文本</p>".into();
    }
    let max = wf.overall.first().map(|(_, c)| *c).filter(|&m| m > 0).unwrap_or(1) as f64;
    let spans: Vec<String> = wf
        .overall
        .iter()
        .map(|(w, c)| {
            let ratio = *c as f64 / max;
            let px = 14.0 + ratio * 32.0;
            let alpha = 0.55 + ratio * 0.45;
            format!(
                "<span style=\"font-size:{:.0}px;opacity:{:.2}\">{} <i>{}</i></span>",
                px, alpha, esc(w), fmt(*c),
            )
        })
        .collect();
    format!("<div class=\"cloud\">{}</div>", spans.join("\n"))
}

fn keywords_html(r: &CoupleReport) -> String {
    let Some(t) = &r.text else {
        return String::new();
    };
    if t.keywords.is_empty() {
        return String::new();
    }
    let rows: Vec<String> = t
        .keywords
        .iter()
        .take(12)
        .map(|(k, me, oth)| {
            format!(
                "<tr><td>{}</td><td class=\"me\">{}</td><td class=\"them\">{}</td></tr>",
                esc(k), fmt(*me), fmt(*oth),
            )
        })
        .collect();
    format!(
        "<section><h2>💬 关键词（消息条数）</h2><table class=\"kw\"><thead><tr><th>词</th><th>我</th><th>ta</th></tr></thead><tbody>{}</tbody></table></section>",
        rows.join(""),
    )
}

fn topwords_html(r: &CoupleReport) -> String {
    let Some(wf) = &r.words else {
        return String::new();
    };
    format!(
        "<section class=\"topwords\"><div><h3>🧑 你最常说</h3><div class=\"chips\">{}</div></div><div><h3>💜 ta 最常说</h3><div class=\"chips\">{}</div></div></section>",
        chips(&wf.mine), chips(&wf.theirs),
    )
}

fn chips(words: &[(String, i64)]) -> String {
    if words.is_empty() {
        return "<span class='muted'>无</span>".into();
    }
    words
        .iter()
        .map(|(w, c)| format!("<span class=\"chip\">{}<i>{}</i></span>", esc(w), fmt(*c)))
        .collect::<Vec<_>>()
        .join("")
}

fn dig_html(r: &CoupleReport) -> String {
    let Some(d) = &r.dig else {
        return String::new();
    };
    let lat = |s: &crate::dig::LatencyStats| -> String {
        match (s.median_sec, s.mean_sec) {
            (Some(md), Some(mn)) => format!(
                "<span class=\"lat\"><b>中位 {}</b><i>均值 {}</i><u>{} 次回复</u></span>",
                crate::fmt::fmt_duration(md), crate::fmt::fmt_duration(mn), fmt(s.samples),
            ),
            _ => "<span class='muted'>无样本</span>".into(),
        }
    };
    let total_open = d.days_self_open + d.days_other_open;
    let my_pct = crate::fmt::pct(d.days_self_open, total_open);
    let types = |dist: &[(i64, i64)], side: &str| -> String {
        if dist.is_empty() {
            return "<span class='muted'>—</span>".into();
        }
        dist.iter()
            .take(6)
            .map(|(t, n)| format!("<span class=\"tc {side}\">{}<i>{}</i></span>", crate::schema::type_label(*t), fmt(*n)))
            .collect::<Vec<_>>()
            .join("")
    };
    format!(
        "<section><h2>🔎 深挖</h2><div class=\"dig-row\"><div class=\"dig-cell\"><h4>⏱ 回复时延</h4><div class=\"latpair\"><div><span class=\"lbl me\">我</span>{a}</div><div><span class=\"lbl them\">ta</span>{b}</div></div></div><div class=\"dig-cell\"><h4>🌅 每日首发</h4><div class=\"bar\"><span class=\"me\" style=\"width:{c:.1}%\"></span><span class=\"them\" style=\"width:{e:.1}%\"></span></div><div class=\"bar-legend\"><span>我开口 {f} 天</span><span>ta 开口 {g} 天</span></div></div></div><div class=\"types\"><div><span class=\"lbl me\">我常发</span><div class=\"tchips\">{h}</div></div><div><span class=\"lbl them\">ta 常发</span><div class=\"tchips\">{i}</div></div></div></section>",
        a = lat(&d.my_reply),
        b = lat(&d.other_reply),
        c = my_pct,
        e = 100.0 - my_pct,
        f = fmt(d.days_self_open),
        g = fmt(d.days_other_open),
        h = types(&d.my_type_dist, "me"),
        i = types(&d.other_type_dist, "them"),
    )
}

fn trend_section(weekly: Option<&crate::trend::TrendSeries>, monthly: Option<&crate::trend::TrendSeries>) -> String {
    let w = weekly.map(|t| one_trend_svg(t, "w")).unwrap_or_default();
    let m = monthly.map(|t| one_trend_svg(t, "m")).unwrap_or_default();
    if w.is_empty() && m.is_empty() {
        return String::new();
    }
    format!(
        "<section><h2>📈 热度趋势<span class=\"tr-toggle\"><button type=\"button\" class=\"tr-btn active\" data-show=\"tr-w\">按周</button><button type=\"button\" class=\"tr-btn\" data-show=\"tr-m\">按月</button></span></h2><div id=\"tr-w\" class=\"tr-pane\">{w}</div><div id=\"tr-m\" class=\"tr-pane\" style=\"display:none\">{m}</div></section>",
    )
}

fn one_trend_svg(t: &crate::trend::TrendSeries, suf: &str) -> String {
    let pts = &t.points;
    if pts.len() < 2 {
        return String::new();
    }
    let w = 740.0;
    let h = 180.0;
    let (pl, pr, pt, pb) = (16.0, 16.0, 18.0, 28.0);
    let iw = w - pl - pr;
    let ih = h - pt - pb;
    let max = pts.iter().map(|p| p.count).max().unwrap_or(1).max(1) as f64;
    let n = pts.len();
    let x = |i: usize| pl + (i as f64) * (iw / ((n - 1) as f64));
    let y = |c: i64| pt + ih - (c as f64 / max) * ih;

    let line: String = pts
        .iter()
        .enumerate()
        .map(|(i, p)| format!("{}{:.1} {:.1}", if i == 0 { "M" } else { "L" }, x(i), y(p.count)))
        .collect::<Vec<_>>()
        .join(" ");
    let baseline = pt + ih;
    let area = format!("{} L {:.1} {:.1} L {:.1} {:.1} Z", line, x(n - 1), baseline, x(0), baseline);

    let peak_idx = pts.iter().enumerate().max_by_key(|(_, p)| p.count).map(|(i, _)| i).unwrap_or(0);
    let pk = &pts[peak_idx];
    let peak = format!(
        "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"3\" class=\"pk\"/><text x=\"{:.1}\" y=\"{:.1}\" class=\"pk-l\">{} · {}</text>",
        x(peak_idx), y(pk.count), x(peak_idx), y(pk.count) - 8.0, esc(&pk.label), fmt(pk.count),
    );

    let dots: String = pts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let tip = format!("{} · {} 条", p.label, fmt(p.count));
            format!("<circle class=\"dot\" cx=\"{:.1}\" cy=\"{:.1}\" r=\"9\" data-tip=\"{}\"/>", x(i), y(p.count), esc(&tip))
        })
        .collect();

    let first = pts.first().unwrap();
    let last = pts.last().unwrap();
    let axis = format!(
        "<text x=\"{:.1}\" y=\"{:.1}\" class=\"ax\">{}</text><text x=\"{:.1}\" y=\"{:.1}\" class=\"ax\" text-anchor=\"end\">{}</text>",
        x(0), baseline + 18.0, esc(&first.label), x(n - 1), baseline + 18.0, esc(&last.label),
    );

    format!(
        "<div class=\"tr-cap\">按{kind}看 · <span class=\"tag tag-{tc}\">{tag}</span></div><svg class=\"trend\" viewBox=\"0 0 {w} {h}\" preserveAspectRatio=\"xMidYMid meet\"><defs><linearGradient id=\"tg-{suf}\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\"><stop offset=\"0\" stop-color=\"var(--me)\" stop-opacity=\"0.4\"/><stop offset=\"1\" stop-color=\"var(--me)\" stop-opacity=\"0.04\"/></linearGradient></defs><path d=\"{area}\" fill=\"url(#tg-{suf})\"/><path d=\"{line}\" fill=\"none\" stroke=\"var(--me)\" stroke-width=\"2\" stroke-linejoin=\"round\"/>{dots}{peak}{axis}</svg>",
        suf = suf, kind = t.kind, tag = t.tag, tc = tag_cls(t.tag),
        w = w, h = h, area = area, line = line, dots = dots, peak = peak, axis = axis,
    )
}

fn tag_cls(tag: &str) -> &'static str {
    match tag {
        "升温" => "up",
        "降温" => "down",
        "平稳" => "flat",
        _ => "unk",
    }
}

fn curios_html(c: &crate::curios::Curios) -> String {
    let mut s = String::from("<section><h2>✨ 趣味统计</h2>");

    // 关系温度计
    let bars: String = c
        .score
        .breakdown
        .iter()
        .map(|(k, p, m)| {
            let pct = if *m > 0 { (*p as f64 / *m as f64 * 100.0) as i64 } else { 0 };
            format!("<div class=\"sb\"><span>{}</span><div class=\"sbb\"><i style=\"width:{}%\"></i></div><u>{}/{}</u></div>", esc(k), pct, p, m)
        })
        .collect();
    s.push_str(&format!(
        "<div class=\"score\"><div class=\"score-num\">{}</div><div class=\"score-meta\"><b>{}</b><div class=\"score-bars\">{}</div></div></div>",
        c.score.score, c.score.label, bars,
    ));

    // 第一次纪念日
    let firsts: Vec<String> = c.firsts.iter().filter_map(|f| {
        f.when.as_ref().map(|w| {
            let snip = esc(&f.snippet.clone().unwrap_or_default());
            let tail = if snip.is_empty() { String::new() } else { format!("「{snip}」") };
            format!("<div class=\"mi\"><span class=\"ml\">{}</span><span class=\"md\">{} {} {}</span></div>", esc(&f.label), esc(w), f.who.unwrap_or(""), tail)
        })
    }).collect();
    if !firsts.is_empty() {
        s.push_str(&format!("<div class=\"milestones\"><h4>🏁 第一次</h4>{}</div>", firsts.join("")));
    }

    // 生物钟双线
    s.push_str(&biorhythm_svg(&c.biorhythm));

    // 指标小卡
    let grid = [
        ("💤 最后一句", format!("我 {} · ta {}", fmt(c.ending.my_last_word), fmt(c.ending.other_last_word))),
        ("⏳ 被晾 >6h", format!("我 {} · ta {}", fmt(c.ending.my_left_on_read), fmt(c.ending.other_left_on_read))),
        ("🔁 最长对线", format!("{} 条 · 约 {} 分钟", fmt(c.rally.max_len), fmt(c.rally.duration_min))),
        ("🌙 深夜 emo", format!("我 {} · ta {}", fmt(c.emo.my_count), fmt(c.emo.other_count))),
        ("😂 哈哈指数", format!("我 {} · ta {}", fmt(c.haha.me_total), fmt(c.haha.them_total))),
        ("🥱 敷衍回复", format!("我 {} · ta {}", fmt(c.perfunctory.my_count), fmt(c.perfunctory.other_count))),
    ];
    let cards: String = grid.iter().map(|(t, b)| format!("<div class=\"ccard\"><div class=\"ct\">{}</div><div class=\"cb\">{}</div></div>", t, esc(b))).collect();
    s.push_str(&format!("<div class=\"cgrid\">{}</div>", cards));

    // 口癖
    if !c.signature.mine.is_empty() || !c.signature.theirs.is_empty() {
        s.push_str(&format!(
            "<div class=\"sign\"><div><span class=\"lbl me\">你的口癖</span><div class=\"chips\">{}</div></div><div><span class=\"lbl them\">ta 的口癖</span><div class=\"chips\">{}</div></div></div>",
            sig_chips(&c.signature.mine), sig_chips(&c.signature.theirs),
        ));
    }

    // 最长深夜长文
    if let Some(e) = &c.emo.longest {
        let who = if e.is_self { "我" } else { "ta" };
        s.push_str(&format!(
            "<div class=\"emo\">🌙 最长深夜长文：{} 字（{} · {}）「{}」</div>",
            fmt(e.chars), who, esc(&e.when), esc(&e.snippet),
        ));
    }

    s.push_str(&heatmap_html(&c.heatmap));
    s.push_str("</section>");
    s
}

fn heatmap_html(h: &crate::trend::Heatmap) -> String {
    if h.cells.is_empty() {
        return String::new();
    }
    let n = h.weeks;
    let mut cells = String::new();
    for (date, count) in &h.cells {
        let lvl = level(*count, h.max);
        let tip = format!("{} · {} 条", cn_date(date), if *count > 0 { fmt(*count) } else { "无".into() });
        cells.push_str(&format!(
            "<i class=\"hm-c lv{lvl}\" data-tip=\"{tip}\" data-date=\"{date}\" data-count=\"{count}\"></i>",
            tip = esc(&tip),
        ));
    }
    let mut months = vec![String::new(); n];
    for (col, m) in &h.month_labels {
        if *col < n {
            months[*col] = m.clone();
        }
    }
    let months_html: String = months.iter().map(|m| format!("<i class=\"hm-m\">{}</i>", esc(m))).collect();
    let wd = ["一", "二", "三", "四", "五", "六", "日"];
    let wd_html: String = wd.iter().map(|d| format!("<i class=\"hm-wd\">{}</i>", d)).collect();
    format!(
        "<section><h2>🗓️ 全年热力图（{a} / {s} 天有聊天）</h2><div class=\"hm\"><div class=\"hm-wdc\">{wd}</div><div class=\"hm-right\"><div class=\"hm-months\">{mh}</div><div class=\"hm-grid\">{c}</div></div></div><div class=\"hm-leg\">少 <i class=\"hm-c lv0\"></i><i class=\"hm-c lv1\"></i><i class=\"hm-c lv2\"></i><i class=\"hm-c lv3\"></i><i class=\"hm-c lv4\"></i> 多</div><div class=\"hm-info\" id=\"hm-info\">点击任意格子查看那一天 →</div></section>",
        a = fmt(h.active_days), s = fmt(h.span_days), wd = wd_html, mh = months_html, c = cells,
    )
}

/// "2025-06-22" → "6月22日"。
fn cn_date(s: &str) -> String {
    let mut it = s.split('-');
    let _y = it.next();
    let m: i64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let d: i64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    format!("{m}月{d}日")
}

fn level(count: i64, max: i64) -> i64 {
    if count == 0 || max == 0 {
        return 0;
    }
    let r = count as f64 / max as f64;
    if r < 0.25 {
        1
    } else if r < 0.5 {
        2
    } else if r < 0.75 {
        3
    } else {
        4
    }
}

fn sig_chips(words: &[(String, i64)]) -> String {
    if words.is_empty() {
        return "<span class='muted'>无</span>".into();
    }
    words
        .iter()
        .map(|(w, n)| format!("<span class=\"chip\">{}<i>{}</i></span>", esc(w), fmt(*n)))
        .collect::<Vec<_>>()
        .join("")
}

fn biorhythm_svg(b: &crate::curios::Biorhythm) -> String {
    let w = 720.0;
    let h = 120.0;
    let (pl, pr, pt, pb) = (22.0, 12.0, 14.0, 22.0);
    let iw = w - pl - pr;
    let ih = h - pt - pb;
    let max = b.me.iter().chain(b.them.iter()).copied().max().unwrap_or(1).max(1) as f64;
    let n = 24usize;
    let x = |i: usize| pl + (i as f64) * (iw / ((n - 1) as f64));
    let y = |v: i64| pt + ih - (v as f64 / max) * ih;
    let line = |arr: &[i64; 24]| -> String {
        arr.iter()
            .enumerate()
            .map(|(i, _)| format!("{}{:.1} {:.1}", if i == 0 { "M" } else { "L" }, x(i), y(arr[i])))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let me = line(&b.me);
    let them = line(&b.them);
    let labels = [0usize, 6, 12, 18, 23]
        .iter()
        .map(|&i| format!("<text x=\"{:.1}\" y=\"{:.1}\" class=\"ax\">{}</text>", x(i), pt + ih + 14.0, i))
        .collect::<Vec<_>>()
        .join("");
    format!(
        "<div class=\"bio\"><h4>🕐 聊天生物钟（0–23 点）<span class=\"lg\"><button class=\"lg-i me\" data-line=\"me\" type=\"button\">● 我</button><button class=\"lg-i them\" data-line=\"them\" type=\"button\">● ta</button></span></h4><svg class=\"trend\" viewBox=\"0 0 {w} {h}\" preserveAspectRatio=\"xMidYMid meet\"><path class=\"bio-me\" d=\"{me}\" fill=\"none\" stroke=\"var(--me)\" stroke-width=\"2\" stroke-linejoin=\"round\"/><path class=\"bio-them\" d=\"{them}\" fill=\"none\" stroke=\"var(--them)\" stroke-width=\"2\" stroke-linejoin=\"round\"/>{labels}</svg></div>",
        w = w, h = h, me = me, them = them, labels = labels,
    )
}

// —— 小工具 ——（数字格式化保留本地 fmt；时长/类型名/百分比统一走 fmt、schema 模块）
fn fmt(n: i64) -> String { format!("{}", n) }
fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// dashboard 与 slides 共用的基础样式。
const BASE_CSS: &str = r#"
  :root { --me:#5b8def; --them:#e85d75; --me-bg:#e3edff; --them-bg:#fde2e8; --bg:#fff7f9; --card:#fff; --ink:#3a2b2e; --mut:#9a8a8d; }
  * { box-sizing:border-box; }
  body { margin:0; font-family:-apple-system,"PingFang SC","Microsoft YaHei",sans-serif; color:var(--ink); }
  section { background:var(--card); border-radius:16px; padding:18px 20px; margin:16px 0; box-shadow:0 4px 16px rgba(0,0,0,.06); }
  section h2 { margin:0 0 12px; font-size:17px; }
  .bar { display:flex; height:26px; border-radius:13px; overflow:hidden; box-shadow:inset 0 0 0 1px #eee; }
  .bar .me { background:var(--me); } .bar .them { background:var(--them); }
  .bar-legend { display:flex; justify-content:space-between; font-size:13px; color:var(--mut); margin-top:6px; }
  .reply-row { display:flex; align-items:center; gap:14px; margin:10px 0; }
  .reply-label { width:96px; font-size:13px; color:var(--mut); }
  .reply-bars { flex:1; display:flex; flex-direction:column; gap:6px; }
  .rbi { display:flex; align-items:center; gap:8px; }
  .who { width:18px; font-size:12px; font-weight:700; }
  .who.me { color:var(--me); } .who.them { color:var(--them); }
  .rb { position:relative; flex:1; height:14px; background:#f0eef0; border-radius:7px; overflow:hidden; }
  .rb span { display:block; height:100%; }
  .rb .me { background:var(--me); } .rb .them { background:var(--them); }
  .rbi i { width:48px; text-align:right; font-style:normal; font-size:12px; color:var(--ink); }
  .cloud { line-height:2.2; text-align:center; word-break:break-word; }
  .cloud i { font-style:normal; font-size:11px; color:var(--mut); vertical-align:super; }
  .kw { width:100%; border-collapse:collapse; font-size:14px; }
  .kw th,.kw td { padding:6px 10px; text-align:left; border-bottom:1px solid #f3eef0; }
  .kw th { color:var(--mut); font-weight:600; }
  .kw .me { color:var(--me); } .kw .them { color:var(--them); }
  .dig-row { display:grid; grid-template-columns:1fr 1fr; gap:20px; }
  .dig-cell h4 { margin:0 0 10px; font-size:14px; }
  .latpair { display:flex; flex-direction:column; gap:8px; }
  .latpair>div { display:flex; align-items:center; gap:8px; }
  .lat { display:inline-flex; align-items:baseline; gap:8px; font-size:14px; }
  .lat b { font-size:15px; } .lat i { font-style:normal; color:var(--mut); font-size:12px; }
  .lat u { text-decoration:none; font-size:11px; color:var(--mut); }
  .lbl { font-size:12px; font-weight:600; padding:2px 8px; border-radius:6px; }
  .lbl.me { color:var(--me); background:var(--me-bg); } .lbl.them { color:var(--them); background:var(--them-bg); }
  .types { margin-top:16px; display:grid; grid-template-columns:1fr 1fr; gap:16px; }
  .types>div { display:flex; flex-direction:column; gap:6px; }
  .tchips { display:flex; flex-wrap:wrap; gap:6px; }
  .tc { padding:4px 10px; border-radius:8px; background:#f3eff2; font-size:13px; }
  .tc.me { background:var(--me-bg); color:var(--me); } .tc.them { background:var(--them-bg); color:var(--them); }
  .tc i { font-style:normal; font-size:11px; opacity:.6; margin-left:4px; }
  .trend { width:100%; height:auto; display:block; }
  .trend .dot { fill:transparent; }
  .trend .pk { fill:var(--me); stroke:#fff; stroke-width:1.5; }
  .trend .pk-l { font-size:11px; fill:var(--ink); text-anchor:middle; font-weight:600; }
  .trend .ax { font-size:11px; fill:var(--mut); }
  .tag { font-size:12px; font-weight:600; padding:3px 10px; border-radius:999px; margin-left:8px; vertical-align:middle; }
  .tag-up { background:var(--them-bg); color:var(--them); }
  .tag-down { background:var(--me-bg); color:var(--me); }
  .tag-flat, .tag-unk { background:#f0eef0; color:var(--mut); }
  .score { display:flex; align-items:center; gap:18px; }
  .score-num { font-size:48px; font-weight:800; color:var(--them); line-height:1; }
  .score-meta b { font-size:16px; }
  .score-bars { margin-top:6px; display:flex; flex-direction:column; gap:4px; min-width:280px; }
  .sb { display:flex; align-items:center; gap:8px; font-size:12px; }
  .sb>span { width:64px; color:var(--mut); }
  .sbb { flex:1; height:8px; background:#f0eef0; border-radius:4px; overflow:hidden; }
  .sbb i { display:block; height:100%; background:var(--them); }
  .sb u { text-decoration:none; color:var(--mut); width:44px; text-align:right; }
  .milestones { margin:14px 0; }
  .milestones h4, .bio h4 { margin:0 0 8px; font-size:14px; }
  .mi { display:flex; gap:10px; padding:5px 0; font-size:13px; border-bottom:1px solid #f5f0f2; }
  .ml { width:150px; color:var(--mut); }
  .md { flex:1; }
  .bio { margin:14px 0; }
  .lg { float:right; font-size:12px; font-weight:400; color:var(--mut); }
  .lg i { display:inline-block; width:14px; height:3px; vertical-align:middle; margin:0 4px 0 8px; border-radius:2px; }
  .lg i.me { background:var(--me); } .lg i.them { background:var(--them); }
  .cgrid { display:grid; grid-template-columns:repeat(3,1fr); gap:10px; margin:14px 0; }
  .ccard { background:#faf7f9; border-radius:10px; padding:10px 12px; }
  .ct { font-size:12px; color:var(--mut); margin-bottom:4px; }
  .cb { font-size:14px; font-weight:600; }
  .sign { display:grid; grid-template-columns:1fr 1fr; gap:16px; margin:10px 0; }
  .sign .lbl { display:inline-block; margin-bottom:6px; }
  .emo { background:#faf7f9; border-radius:10px; padding:12px 14px; font-size:13px; margin-top:10px; line-height:1.6; }
  .lg-i { border:none; background:none; cursor:pointer; font-size:12px; font-weight:600; margin-left:10px; }
  .lg-i.me { color:var(--me); } .lg-i.them { color:var(--them); }
  .lg-i.off { opacity:.3; text-decoration:line-through; }
  .bio-me.hide, .bio-them.hide { opacity:0; transition:opacity .2s; }
  body.dark { --card:#241b1d; --ink:#efe7e9; --mut:#9b8c8f; }
  body.dark .ccard, body.dark .emo { background:#2a2022; }
  body.dark .hm-c.lv0 { background:#33292b; }
  .hm { display:flex; gap:6px; overflow-x:auto; padding:4px 0; }
  .hm-wdc { display:grid; grid-template-rows:repeat(7,11px); gap:2px; padding-top:18px; }
  .hm-wd { font-size:9px; color:var(--mut); line-height:11px; }
  .hm-months { display:grid; grid-auto-flow:column; grid-auto-columns:11px; gap:2px; height:14px; margin-bottom:3px; }
  .hm-m { font-size:9px; color:var(--mut); white-space:nowrap; }
  .hm-grid { display:grid; grid-template-rows:repeat(7,11px); grid-auto-flow:column; grid-auto-columns:11px; gap:2px; }
  .hm-c { width:11px; height:11px; border-radius:2px; display:block; }
  .hm-c.lv0 { background:#eee; } .hm-c.lv1 { background:#fcd5dd; } .hm-c.lv2 { background:#f59caf; }
  .hm-c.lv3 { background:#e85d75; } .hm-c.lv4 { background:#c0354f; }
  .hm-leg { font-size:11px; color:var(--mut); margin-top:10px; display:flex; align-items:center; gap:3px; }
  .hm-leg .hm-c { width:11px; height:11px; margin:0 1px; }
  .hm-info { margin-top:10px; font-size:13px; color:var(--mut); background:#faf7f9; border-radius:8px; padding:8px 12px; }
  body.dark .hm-info { background:#2a2022; }
  .tr-toggle { float:right; font-size:13px; font-weight:400; }
  .tr-btn { border:1px solid #e6dfe2; background:var(--card); color:var(--mut); padding:3px 12px; cursor:pointer; border-radius:999px; font-size:12px; margin-left:4px; }
  .tr-btn.active { background:var(--me); color:#fff; border-color:var(--me); }
  .tr-cap { font-size:13px; color:var(--mut); margin-bottom:4px; }
  .topwords { display:grid; grid-template-columns:1fr 1fr; gap:20px; }
  .topwords h3 { margin:0 0 10px; font-size:15px; }
  .chips { display:flex; flex-wrap:wrap; gap:8px; }
  .chip { padding:5px 11px; border-radius:999px; font-size:13px; background:#f1eef2; }
  .chip i { font-style:normal; font-size:11px; opacity:.6; margin-left:4px; }
  .chip.me { background:var(--me-bg); } .chip.them { background:var(--them-bg); }
  .muted { color:var(--mut); font-size:14px; }
"#;

/// dashboard 专属的页面布局样式（header/wrap/cards/footer 等）。
const DASHBOARD_CSS: &str = r#"
  body { background:linear-gradient(180deg,#fff0f3,#f4f7ff); padding:32px 16px 64px; }
  body.dark { background:linear-gradient(180deg,#1f1719,#161a22); }
  .wrap { max-width:1040px; margin:0 auto; }
  header { text-align:center; margin-bottom:8px; position:relative; }
  header h1 { margin:0 0 6px; font-size:26px; }
  header .meta { color:var(--mut); font-size:14px; }
  #theme { position:absolute; top:0; right:0; border:none; background:var(--card); border-radius:999px; width:40px; height:40px; font-size:18px; cursor:pointer; box-shadow:0 2px 10px rgba(0,0,0,.08); }
  #tip { position:fixed; pointer-events:none; background:#3a2b2e; color:#fff; padding:4px 9px; border-radius:6px; font-size:12px; opacity:0; transition:opacity .12s; z-index:99; white-space:nowrap; }
  .pills { display:flex; gap:10px; justify-content:center; margin:14px 0 24px; }
  .pill { background:var(--card); border-radius:999px; padding:8px 16px; font-weight:600; box-shadow:0 2px 10px rgba(0,0,0,.06); font-size:14px; }
  .cards { display:grid; grid-template-columns:repeat(3,1fr); gap:12px; margin:20px 0; }
  .card { background:var(--card); border-radius:16px; padding:16px 14px; text-align:center; box-shadow:0 4px 16px rgba(0,0,0,.06); }
  .card-val { font-size:22px; font-weight:700; }
  .card-label { font-size:13px; color:var(--mut); margin-top:4px; }
  .card-sub { font-size:12px; color:var(--mut); margin-top:2px; }
  footer { text-align:center; color:var(--mut); font-size:12px; margin-top:24px; }
  @media(max-width:560px){ .cards,.cgrid{grid-template-columns:repeat(2,1fr)} .topwords,.dig-row,.types,.sign{grid-template-columns:1fr} }
"#;

/// 幻灯片（PPT）模式专用样式：全屏翻页。
const DECK_CSS: &str = r#"
  html,body { height:100%; }
  body { background:linear-gradient(135deg,#fff0f3 0%,#f4f7ff 100%); overflow:hidden; }
  body.dark { background:linear-gradient(135deg,#1f1719,#161a22); }
  .deck { height:100vh; width:100vw; position:relative; overflow:hidden; }
  .track { display:flex; height:100%; transition:transform .55s cubic-bezier(.22,.61,.36,1); will-change:transform; }
  .slide { min-width:100vw; height:100vh; display:flex; flex-direction:column; justify-content:center; align-items:center; padding:7vh 8vw; overflow:auto; text-align:center; }
  .slide h2 { font-size:30px; margin:0 0 26px; }
  .slide section { width:100%; max-width:900px; margin:0 auto; text-align:left; }
  .slide .emoji { font-size:72px; }
  .s-cover h1 { font-size:44px; margin:10px 0 6px; }
  .s-cover .sub { color:var(--mut); font-size:18px; margin:0 0 14px; }
  .s-cover .big { font-size:50px; font-weight:800; color:var(--them); margin:0; }
  .s-cover .hint { color:var(--mut); font-size:13px; margin-top:34px; }
  .s-score { font-size:128px; font-weight:800; color:var(--them); line-height:1; }
  .s-score span { font-size:32px; color:var(--mut); }
  .s-scorelabel { font-size:26px; margin:6px 0 24px; }
  .s-stats { display:flex; gap:24px; flex-wrap:wrap; justify-content:center; }
  .s-stat { background:var(--card); border-radius:18px; padding:22px 30px; text-align:center; box-shadow:0 4px 16px rgba(0,0,0,.06); min-width:150px; }
  .s-stat b { display:block; font-size:40px; color:var(--me); line-height:1.1; }
  .s-stat span { color:var(--mut); font-size:14px; }
  .s-note { color:var(--mut); font-size:16px; margin-top:22px; }
  .slide .bar { max-width:560px; width:100%; }
  .nav { position:fixed; top:50%; transform:translateY(-50%); width:100%; display:flex; justify-content:space-between; padding:0 18px; pointer-events:none; z-index:20; }
  .nav button { pointer-events:auto; border:none; background:var(--card); width:50px; height:50px; border-radius:50%; font-size:26px; cursor:pointer; box-shadow:0 2px 12px rgba(0,0,0,.12); color:var(--ink); }
  .dots { position:fixed; bottom:18px; left:50%; transform:translateX(-50%); display:flex; gap:7px; z-index:20; }
  .dots i { width:8px; height:8px; border-radius:50%; background:#d8c8cc; cursor:pointer; transition:all .3s; }
  .dots i.on { background:var(--them); width:24px; border-radius:4px; }
  .counter { position:fixed; top:20px; right:24px; color:var(--mut); font-size:14px; z-index:20; }
  .progress { position:fixed; top:0; left:0; height:3px; background:var(--them); z-index:21; transition:width .4s; }
  #theme { position:fixed; top:16px; left:22px; border:none; background:var(--card); border-radius:999px; width:40px; height:40px; font-size:18px; cursor:pointer; box-shadow:0 2px 10px rgba(0,0,0,.1); z-index:20; }
"#;

/// 幻灯片外壳：{{CSS}}=BASE+DECK，{{TITLE}}，{{SLIDES}}，{{DOTS}}。
const SLIDES_SHELL: &str = r#"<!DOCTYPE html>
<html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{{TITLE}}</title>
<style>{{CSS}}</style></head>
<body>
<button id="theme" type="button" title="切换深色">🌙</button>
<div class="counter" id="counter"></div>
<div class="progress" id="progress"></div>
<div class="dots" id="dots">{{DOTS}}</div>
<div class="nav"><button id="prev" type="button">‹</button><button id="next" type="button">›</button></div>
<div class="deck" id="deck"><div class="track" id="track">{{SLIDES}}</div></div>
<script>
(function(){
  var tip=document.createElement('div'); tip.id='tip'; tip.style.cssText='position:fixed;pointer-events:none;background:#3a2b2e;color:#fff;padding:4px 9px;border-radius:6px;font-size:12px;opacity:0;transition:opacity .12s;z-index:99;white-space:nowrap;'; document.body.appendChild(tip);
  document.addEventListener('mouseover',function(e){var el=e.target.closest('[data-tip]');if(!el)return;tip.textContent=el.getAttribute('data-tip');tip.style.opacity='1';});
  document.addEventListener('mousemove',function(e){tip.style.left=(e.clientX+14)+'px';tip.style.top=(e.clientY+14)+'px';});
  document.addEventListener('mouseout',function(e){if(e.target.closest('[data-tip]'))tip.style.opacity='0';});
  var t=document.getElementById('theme'); if(t)t.onclick=function(){document.body.classList.toggle('dark');t.textContent=document.body.classList.contains('dark')?'☀️':'🌙';};
  document.querySelectorAll('.lg-i').forEach(function(b){b.onclick=function(){var line=b.getAttribute('data-line');var p=document.querySelector('.bio-'+line);if(p)p.classList.toggle('hide');b.classList.toggle('off');};});
  document.querySelectorAll('.tr-btn').forEach(function(b){b.onclick=function(){document.querySelectorAll('.tr-pane').forEach(function(p){p.style.display='none';});var pane=document.getElementById(b.getAttribute('data-show'));if(pane)pane.style.display='';document.querySelectorAll('.tr-btn').forEach(function(x){x.classList.remove('active');});b.classList.add('active');};});
  var wdNames=['日','一','二','三','四','五','六'];document.querySelectorAll('.hm-c[data-date]').forEach(function(c){c.style.cursor='pointer';c.onclick=function(){var d=c.getAttribute('data-date');var n=parseInt(c.getAttribute('data-count')||'0',10);var dt=new Date(d+'T00:00:00');var wd=isNaN(dt)?'':' 周'+wdNames[dt.getDay()];console.log(d.replace(/-/g,'/')+wd+' · '+n+' 条');};});
  var i=0,N=document.querySelectorAll('.slide').length,track=document.getElementById('track'),counter=document.getElementById('counter'),progress=document.getElementById('progress'),dots=document.querySelectorAll('#dots i');
  function render(){track.style.transform='translateX(-'+i*100+'%)';counter.textContent=(i+1)+' / '+N;progress.style.width=((i+1)/N*100)+'%';dots.forEach(function(d,k){d.classList.toggle('on',k===i);});}
  function go(n){i=Math.max(0,Math.min(N-1,n));render();}
  go(0);
  document.getElementById('prev').onclick=function(){go(i-1);};document.getElementById('next').onclick=function(){go(i+1);};
  dots.forEach(function(d,k){d.onclick=function(){go(k);};});
  document.addEventListener('keydown',function(e){if(['ArrowRight','ArrowDown',' ','PageDown'].indexOf(e.key)>=0){e.preventDefault();go(i+1);}else if(['ArrowLeft','ArrowUp','PageUp'].indexOf(e.key)>=0){e.preventDefault();go(i-1);}else if(e.key==='Home'){go(0);}else if(e.key==='End'){go(N-1);}});
  var lock=false;document.getElementById('deck').addEventListener('wheel',function(e){if(lock)return;lock=true;setTimeout(function(){lock=false;},650);if(e.deltaY>10)go(i+1);else if(e.deltaY<-10)go(i-1);},{passive:true});
  document.getElementById('deck').addEventListener('click',function(e){if(e.target.closest('button,.dots,#theme,.tr-toggle,.lg,.hm-grid,.hm-c,.kw,.cloud,.chip'))return;var x=e.clientX/window.innerWidth;go(x<0.3?i-1:i+1);});
  var sx=0,sy=0;document.getElementById('deck').addEventListener('touchstart',function(e){sx=e.touches[0].clientX;sy=e.touches[0].clientY;},{passive:true});document.getElementById('deck').addEventListener('touchend',function(e){var dx=e.changedTouches[0].clientX-sx,dy=e.changedTouches[0].clientY-sy;if(Math.abs(dx)>50&&Math.abs(dx)>Math.abs(dy))go(dx<0?i+1:i-1);},{passive:true});
})();
</script>
</body></html>"#;

const HTML_TEMPLATE: &str = r#"<!DOCTYPE html>
<html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{{TITLE}}</title>
<style>
{{CSS}}
</style></head>
<body><div class="wrap">
  <header><button id="theme" type="button" title="切换深色">🌙</button><h1>{{TITLE}}</h1><div class="meta">{{SPAN}}</div></header>
  <div class="pills"><span class="pill">📅 第 {{NTH}} 天</span><span class="pill">💌 {{TOTAL}} 条消息</span></div>
  {{CARDS}}
  <section><h2>📊 发送比例</h2>{{RATIO}}</section>
  <section><h2>⚡ 回复速度</h2>{{REPLY}}</section>
  {{TREND}}
  <section><h2>☁️ 高频词云</h2>{{CLOUD}}</section>
  {{DIG}}
  {{KEYWORDS}}
  {{TOPWORDS}}
  {{CURIOS}}
  <footer>由 wechat-statistics 本地生成 · 全程离线，数据未离开本机</footer>
</div>
<script>
(function(){
  var tip=document.createElement('div'); tip.id='tip'; document.body.appendChild(tip);
  document.addEventListener('mouseover',function(e){
    var el=e.target.closest('[data-tip]'); if(!el){return;}
    tip.textContent=el.getAttribute('data-tip'); tip.style.opacity='1';
  });
  document.addEventListener('mousemove',function(e){
    tip.style.left=(e.clientX+14)+'px'; tip.style.top=(e.clientY+14)+'px';
  });
  document.addEventListener('mouseout',function(e){
    if(e.target.closest('[data-tip]')){tip.style.opacity='0';}
  });
  var t=document.getElementById('theme');
  if(t){t.onclick=function(){document.body.classList.toggle('dark');t.textContent=document.body.classList.contains('dark')?'☀️':'🌙';};}
  document.querySelectorAll('.lg-i').forEach(function(b){
    b.onclick=function(){
      var line=b.getAttribute('data-line');
      var p=document.querySelector('.bio-'+line);
      if(p){p.classList.toggle('hide');}
      b.classList.toggle('off');
    };
  });
  document.querySelectorAll('.tr-btn').forEach(function(b){
    b.onclick=function(){
      document.querySelectorAll('.tr-pane').forEach(function(p){p.style.display='none';});
      var pane=document.getElementById(b.getAttribute('data-show'));
      if(pane){pane.style.display='';}
      document.querySelectorAll('.tr-btn').forEach(function(x){x.classList.remove('active');});
      b.classList.add('active');
    };
  });
  var wdNames=['日','一','二','三','四','五','六'];
  var info=document.getElementById('hm-info');
  document.querySelectorAll('.hm-c[data-date]').forEach(function(c){
    c.style.cursor='pointer';
    c.onclick=function(){
      if(!info){return;}
      var d=c.getAttribute('data-date'); var n=parseInt(c.getAttribute('data-count')||'0',10);
      var dt=new Date(d+'T00:00:00');
      var wd=isNaN(dt)?'':' 周'+wdNames[dt.getDay()];
      var nice=d.replace(/-/g,'/');
      info.innerHTML='📅 <b>'+nice+'</b>'+wd+' · '+(n>0?'<b>'+n+'</b> 条消息':'这天没聊天');
    };
  });
})();
</script>
</body></html>"#;
