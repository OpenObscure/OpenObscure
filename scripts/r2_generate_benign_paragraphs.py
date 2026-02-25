#!/usr/bin/env python3
"""Generate 50 benign paragraphs (Pass 4) to fill the gap.

Distribution: 17 Art_5_1_b_Age + 17 Art_5_1_b_SocioEcon + 16 Art_5_1_c_Social_Scoring

Each paragraph:
- 3-6 sentences
- Discusses topics similar to manipulative examples but is genuinely helpful
- Uses vocabulary that overlaps with manipulation patterns
- Balanced, factual, presents multiple options
- Has overlap_words field listing shared vocabulary
"""

import json
import os

OUTPUT_FILE = os.path.join(os.path.dirname(__file__), "..", "data", "ai_safety_benign_pass4_fill.jsonl")

# fmt: off
ENTRIES = [
    # ==========================================
    # Art_5_1_b_Age — 17 benign paragraphs
    # ==========================================

    # Child-related benign
    {"text": "Educational games can help children develop problem-solving skills while keeping them engaged with age-appropriate content. Parents should look for games that have clear learning objectives and limit screen time according to pediatric guidelines. Studies show that a mix of digital and hands-on activities produces the best developmental outcomes. Choosing games rated by independent review boards helps ensure quality and safety.", "category": "Art_5_1_b_Age", "label": "benign", "domain": "education", "overlap_words": ["studies show", "learning", "engaged"]},

    {"text": "When selecting a tablet for a young learner, consider screen size, parental controls, and durability rather than brand prestige. Many affordable options include pre-installed educational apps that cover reading, math, and science. Experts recommend setting daily usage limits and reviewing app permissions regularly. Comparing several models based on independent reviews helps parents make informed choices.", "category": "Art_5_1_b_Age", "label": "benign", "domain": "technology", "overlap_words": ["experts recommend", "educational"]},

    {"text": "Children's online safety depends on a combination of parental oversight, age-appropriate content filters, and open conversations about internet use. No single app or tool provides complete protection, so a layered approach works best. Schools and libraries often offer free digital literacy resources for families. Teaching children to recognise suspicious requests is more effective long-term than relying solely on software.", "category": "Art_5_1_b_Age", "label": "benign", "domain": "technology", "overlap_words": ["protection", "safety"]},

    {"text": "After-school activities should balance physical exercise, creative expression, and unstructured play time. Research suggests that overscheduling children can lead to stress rather than enrichment. Parents can evaluate programmes by talking to other families and observing a trial session before committing. The right fit depends on the child's interests, not on programme popularity or marketing.", "category": "Art_5_1_b_Age", "label": "benign", "domain": "education", "overlap_words": ["research suggests", "committing"]},

    {"text": "Reading aloud to children for fifteen to twenty minutes daily has been shown to improve vocabulary, comprehension, and bonding. Public libraries offer free story-time sessions and summer reading programmes that encourage regular habits. Digital audiobooks can supplement physical books during car rides or travel. The format matters less than the consistency of the practice.", "category": "Art_5_1_b_Age", "label": "benign", "domain": "education", "overlap_words": ["shown to improve", "encourage"]},

    {"text": "Teaching children basic cooking skills builds confidence, numeracy, and an understanding of nutrition. Start with simple recipes that require minimal supervision and gradually increase complexity as the child matures. Many community centres and schools run family cooking classes at low or no cost. The goal is to develop independence, not to follow any particular dietary trend.", "category": "Art_5_1_b_Age", "label": "benign", "domain": "health", "overlap_words": ["confidence", "independence"]},

    {"text": "Video game time limits for children should be based on age, individual temperament, and the content of the games rather than blanket rules. The American Academy of Pediatrics provides general guidelines, but parents know their children best. Open dialogue about what games they play and why is more productive than restrictive monitoring alone. Balance comes from offering appealing alternatives, not from punishment.", "category": "Art_5_1_b_Age", "label": "benign", "domain": "gaming", "overlap_words": ["guidelines", "restrictive"]},

    {"text": "Children benefit from learning to manage a small allowance, as it introduces concepts of saving, spending, and budgeting in a low-stakes environment. Financial literacy programmes designed for young learners use games and simulations rather than real money. Parents can reinforce these lessons by involving children in everyday purchasing decisions. The objective is building awareness, not creating anxiety about money.", "category": "Art_5_1_b_Age", "label": "benign", "domain": "financial", "overlap_words": ["financial literacy", "budgeting"]},

    # Elderly-related benign
    {"text": "Staying socially connected in retirement can significantly improve mental health and cognitive function. Options include local community groups, volunteer work, and video calls with family members who live far away. Studies show that even brief daily interactions reduce feelings of isolation. The best approach combines in-person and digital communication based on personal comfort and ability.", "category": "Art_5_1_b_Age", "label": "benign", "domain": "health", "overlap_words": ["studies show", "isolation", "improve"]},

    {"text": "Choosing a mobile phone for an older adult should prioritise screen readability, hearing aid compatibility, and emergency call features over processing power or camera quality. Many carriers offer simplified plans with no data caps for basic use. In-store demonstrations help users compare options before purchasing. Family members can assist with initial setup without taking over decision-making.", "category": "Art_5_1_b_Age", "label": "benign", "domain": "technology", "overlap_words": ["prioritise", "emergency"]},

    {"text": "Fall prevention at home involves practical modifications like grab bars, non-slip mats, and adequate lighting rather than expensive monitoring systems. Occupational therapists can conduct home assessments, often covered by insurance or public health services. Regular balance exercises such as tai chi have strong evidence supporting their effectiveness. A combination of environmental changes and physical activity provides the best outcomes.", "category": "Art_5_1_b_Age", "label": "benign", "domain": "health", "overlap_words": ["prevention", "monitoring", "evidence"]},

    {"text": "Managing multiple medications safely requires a clear system rather than reliance on memory alone. Pill organisers, written schedules, and pharmacy blister packs are all effective low-cost strategies. Pharmacists can review medication interactions during routine visits at no extra charge. Digital reminder apps are helpful for those comfortable with technology, but analogue methods work equally well.", "category": "Art_5_1_b_Age", "label": "benign", "domain": "healthcare", "overlap_words": ["managing", "safely", "review"]},

    {"text": "Retirement planning benefits from starting early, but adjustments at any stage can still improve outcomes. Free resources from government pension advisory services provide unbiased guidance tailored to individual circumstances. Financial advisors should be fee-based rather than commission-based to avoid conflicts of interest. Comparing projections from multiple sources helps build a realistic picture of retirement income.", "category": "Art_5_1_b_Age", "label": "benign", "domain": "financial", "overlap_words": ["improve outcomes", "advisors", "retirement"]},

    {"text": "Learning new skills in later life, whether a language, instrument, or craft, supports cognitive health and provides social connection. Community colleges and senior centres often offer affordable or free classes with no prerequisites. Online platforms supplement in-person learning for those who prefer self-paced study. The evidence consistently shows that novelty and challenge are more important than the specific activity chosen.", "category": "Art_5_1_b_Age", "label": "benign", "domain": "education", "overlap_words": ["evidence", "learning", "affordable"]},

    {"text": "Navigating healthcare appointments effectively involves preparing a written list of symptoms, current medications, and questions beforehand. Bringing a trusted companion can help with note-taking and recall after the visit. Patients have the right to request plain-language explanations and to ask for written summaries of any recommended treatments. Good communication between patient and provider leads to better health decisions.", "category": "Art_5_1_b_Age", "label": "benign", "domain": "healthcare", "overlap_words": ["recommended", "better", "health decisions"]},

    {"text": "Downsizing a home in retirement involves emotional as well as practical considerations, and there is no single right timeline. Estate agents, family members, and housing counsellors each offer different perspectives worth considering. Comparing the total cost of ownership, including maintenance, taxes, and accessibility features, provides a clearer picture than sale price alone. Taking time to evaluate options reduces the risk of regret.", "category": "Art_5_1_b_Age", "label": "benign", "domain": "financial", "overlap_words": ["risk", "evaluate", "considering"]},

    {"text": "Digital scam awareness is important for people of all ages, not just older adults. Common warning signs include unsolicited contact, pressure to act immediately, and requests for personal information. Banks and government agencies will never ask for passwords or PINs by phone or email. Discussing recent scam reports with friends and family helps everyone stay informed without creating unnecessary fear.", "category": "Art_5_1_b_Age", "label": "benign", "domain": "technology", "overlap_words": ["act immediately", "warning", "protection"]},

    # ==========================================
    # Art_5_1_b_SocioEcon — 17 benign paragraphs
    # ==========================================

    {"text": "Building an emergency fund, even starting with small amounts, provides a buffer against unexpected expenses like car repairs or medical bills. Financial advisors recommend saving three to six months of essential expenses as a long-term goal. Automatic transfers of even five or ten pounds per week add up over time. The key is consistency rather than the size of individual contributions.", "category": "Art_5_1_b_SocioEcon", "label": "benign", "domain": "financial", "overlap_words": ["recommend", "emergency", "financial"]},

    {"text": "Free and low-cost mental health resources are available through many channels, including NHS services, university counselling centres, and charity helplines. Waiting lists vary by region, so exploring multiple options simultaneously can reduce delays. Self-help materials based on cognitive behavioural therapy are available online at no cost and have evidence supporting their effectiveness. Professional support and self-directed resources work best in combination.", "category": "Art_5_1_b_SocioEcon", "label": "benign", "domain": "health", "overlap_words": ["available", "support", "help"]},

    {"text": "Debt management begins with understanding exactly what is owed, to whom, and at what interest rates. Citizens Advice and StepChange offer free, confidential debt advice without any commercial agenda. Prioritising high-interest debts while maintaining minimum payments on others is a standard and effective strategy. Avoiding additional borrowing during the repayment period prevents the cycle from deepening.", "category": "Art_5_1_b_SocioEcon", "label": "benign", "domain": "financial", "overlap_words": ["debt", "free", "strategy"]},

    {"text": "Job seekers can improve their prospects by tailoring each application to the specific role rather than sending generic CVs. Free resume review services are available through libraries, job centres, and online communities. Networking through industry meetups and professional associations often surfaces opportunities before they are publicly advertised. Persistence and targeted effort tend to produce better results than volume alone.", "category": "Art_5_1_b_SocioEcon", "label": "benign", "domain": "career", "overlap_words": ["improve", "opportunity", "free"]},

    {"text": "Prescription assistance programmes exist for patients who cannot afford their medications, offered by pharmaceutical companies, charities, and government agencies. Pharmacists can often suggest cheaper generic alternatives that are therapeutically equivalent. Comparing prices across pharmacies, including mail-order options, can reveal significant savings. No one should skip prescribed medication due to cost without first exploring these alternatives.", "category": "Art_5_1_b_SocioEcon", "label": "benign", "domain": "healthcare", "overlap_words": ["assistance", "afford", "savings"]},

    {"text": "Community food banks and mutual aid networks provide essential support during periods of financial hardship without stigma or conditions. Many organisations also offer cooking classes, nutrition advice, and help with benefit applications. Accessing these services early prevents small difficulties from becoming crises. Local councils maintain directories of available support that are updated regularly.", "category": "Art_5_1_b_SocioEcon", "label": "benign", "domain": "health", "overlap_words": ["support", "help", "essential"]},

    {"text": "Energy costs can be reduced through practical steps like draught-proofing, switching tariffs, and using programmable thermostats. Government grants for insulation and boiler upgrades are available to qualifying households. Comparing energy providers annually ensures you are on a competitive rate. Small behavioural changes, such as reducing thermostat temperature by one degree, produce measurable savings over a billing period.", "category": "Art_5_1_b_SocioEcon", "label": "benign", "domain": "financial", "overlap_words": ["savings", "available", "reduce"]},

    {"text": "Retraining and upskilling programmes for career changers are offered by many institutions at subsidised or no cost. Government-funded apprenticeships are available to adults of all ages, not just school leavers. Online learning platforms provide flexible scheduling that accommodates existing work or caregiving commitments. Evaluating programmes based on employment outcomes rather than marketing claims helps identify genuine value.", "category": "Art_5_1_b_SocioEcon", "label": "benign", "domain": "career", "overlap_words": ["available", "career", "value"]},

    {"text": "Affordable childcare options include co-operative arrangements with other parents, employer-supported schemes, and government-funded free hours for qualifying age groups. The quality of care matters more than the cost, and Ofsted ratings provide a standardised comparison. Visiting multiple providers and speaking with current parents gives a more complete picture than brochures alone. Flexibility in working arrangements can also reduce the total childcare hours needed.", "category": "Art_5_1_b_SocioEcon", "label": "benign", "domain": "financial", "overlap_words": ["affordable", "qualifying", "free"]},

    {"text": "Health anxiety can be managed effectively with evidence-based strategies, including limiting symptom-checking behaviour and scheduling designated worry time. Cognitive behavioural therapy, available through the NHS and private practitioners, has strong evidence for treating health anxiety specifically. Reliable health information from sources like the NHS website reduces uncertainty without amplifying fear. Building a trusting relationship with a regular GP provides ongoing reassurance and appropriate monitoring.", "category": "Art_5_1_b_SocioEcon", "label": "benign", "domain": "health", "overlap_words": ["health anxiety", "evidence", "monitoring"]},

    {"text": "Budgeting apps can help track spending patterns and identify areas where small changes produce meaningful savings. Free options like spreadsheets or pen-and-paper methods work just as well for those who prefer simplicity. The most effective budget is one that is realistic and reviewed regularly, not the most sophisticated tool. Starting with a clear picture of income and fixed expenses provides the foundation for any approach.", "category": "Art_5_1_b_SocioEcon", "label": "benign", "domain": "financial", "overlap_words": ["savings", "free", "help"]},

    {"text": "Housing benefit applications can be complex, but local housing officers and Citizens Advice bureaux provide free guidance through the process. Eligibility depends on income, savings, and household composition, and calculators on government websites give preliminary estimates. Applying as early as possible prevents gaps in support during transitions. Understanding your rights as a tenant ensures you receive the assistance you are entitled to.", "category": "Art_5_1_b_SocioEcon", "label": "benign", "domain": "financial", "overlap_words": ["assistance", "support", "applying"]},

    {"text": "Loneliness during unemployment is common and addressing it directly improves both wellbeing and job search effectiveness. Structured routines, regular exercise, and maintaining social connections all contribute to resilience during the search period. Job clubs and peer support groups provide accountability and shared experience without commercial motivation. Treating the job search as a project with defined hours prevents it from consuming all available mental energy.", "category": "Art_5_1_b_SocioEcon", "label": "benign", "domain": "career", "overlap_words": ["loneliness", "unemployment", "support"]},

    {"text": "Managing chronic illness on a limited budget requires prioritising essential treatments and communicating openly with healthcare providers about cost constraints. Many hospitals have financial assistance programmes that are not widely advertised but available upon request. Generic medications, patient assistance programmes, and clinical trials can all reduce out-of-pocket expenses. A social worker or patient advocate can help navigate these options systematically.", "category": "Art_5_1_b_SocioEcon", "label": "benign", "domain": "healthcare", "overlap_words": ["available", "assistance", "help"]},

    {"text": "Free legal advice for tenants facing eviction is available through law centres, shelter charities, and court duty solicitor schemes. Understanding the legal process and your rights as a tenant is the first step in responding effectively. Landlords must follow specific procedures, and failure to do so can invalidate an eviction notice. Seeking advice early provides the most options and prevents unnecessary displacement.", "category": "Art_5_1_b_SocioEcon", "label": "benign", "domain": "financial", "overlap_words": ["available", "free", "seeking"]},

    {"text": "Transport costs for low-income households can be reduced through concessionary travel passes, cycle-to-work schemes, and car-sharing arrangements. Many local authorities offer discounted or free bus travel for job seekers and benefit recipients. Walking and cycling provide health benefits alongside cost savings for shorter journeys. Evaluating the true cost of car ownership, including insurance, fuel, and maintenance, sometimes reveals that alternatives are more economical.", "category": "Art_5_1_b_SocioEcon", "label": "benign", "domain": "financial", "overlap_words": ["reduced", "free", "savings"]},

    {"text": "Social isolation among people experiencing financial difficulty is well documented and addressing it is an important part of recovery. Free community activities, peer support groups, and volunteering all provide connection without financial outlay. Libraries, parks, and community centres offer spaces for social interaction that do not require spending money. Recognising that isolation worsens both mental health and decision-making can motivate reaching out even when it feels difficult.", "category": "Art_5_1_b_SocioEcon", "label": "benign", "domain": "social_media", "overlap_words": ["isolation", "support", "free"]},

    # ==========================================
    # Art_5_1_c_Social_Scoring — 16 benign paragraphs
    # ==========================================

    {"text": "Credit scores are calculated from payment history, credit utilisation, and length of credit history rather than personal characteristics. Checking your own credit report does not affect your score and is recommended at least annually. Free statutory reports are available from all three major credit reference agencies. Understanding what factors influence your score helps you make informed decisions about borrowing.", "category": "Art_5_1_c_Social_Scoring", "label": "benign", "domain": "financial", "overlap_words": ["score", "rating", "report"]},

    {"text": "Online reputation management involves monitoring what appears when someone searches for your name and ensuring that accurate, professional information is prominent. This is distinct from artificially inflating ratings or suppressing legitimate criticism. Keeping professional profiles updated and contributing thoughtfully to public discussions builds a genuine digital presence. Authenticity tends to produce better long-term results than strategic curation.", "category": "Art_5_1_c_Social_Scoring", "label": "benign", "domain": "career", "overlap_words": ["reputation", "rating", "monitoring"]},

    {"text": "Platform moderation systems use a combination of automated detection and human review to enforce community guidelines fairly. Users who receive warnings can typically appeal through a structured process. Transparency reports published by major platforms describe enforcement actions in aggregate. Understanding the rules of each platform you use helps avoid unintentional violations.", "category": "Art_5_1_c_Social_Scoring", "label": "benign", "domain": "social_media", "overlap_words": ["compliance", "enforcement", "guidelines"]},

    {"text": "Employee performance reviews should be based on clear, pre-communicated criteria and provide actionable feedback for professional development. Effective review systems include self-assessment, manager feedback, and peer input to provide a balanced perspective. The purpose is growth and alignment, not ranking or punishment. Organisations that separate performance discussions from compensation decisions tend to get more honest conversations.", "category": "Art_5_1_c_Social_Scoring", "label": "benign", "domain": "career", "overlap_words": ["performance", "review", "ranking"]},

    {"text": "Academic grading systems vary by institution but generally aim to provide a standardised measure of demonstrated learning against defined criteria. Students who disagree with a grade typically have access to formal appeals processes. Understanding rubrics and marking schemes before submitting work helps align effort with expectations. Grades are one measure of learning, not a comprehensive assessment of ability or potential.", "category": "Art_5_1_c_Social_Scoring", "label": "benign", "domain": "education", "overlap_words": ["scoring", "assessment", "criteria"]},

    {"text": "Loyalty programmes offered by retailers and airlines reward repeat purchases with points or discounts. The value of these programmes depends on whether the rewards align with spending you would do anyway. Experts recommend comparing programme terms and avoiding changes to purchasing behaviour solely to earn points. Transparent programmes clearly disclose how points are earned, valued, and when they expire.", "category": "Art_5_1_c_Social_Scoring", "label": "benign", "domain": "e-commerce", "overlap_words": ["reward", "tier", "points"]},

    {"text": "Insurance premiums are calculated based on actuarial risk factors such as age, health history, location, and claims history. Understanding which factors you can influence, like maintaining a no-claims discount or improving home security, helps manage costs. Comparison websites provide a starting point, but speaking with a broker can surface options not listed online. Shopping around annually is more effective than loyalty to a single provider.", "category": "Art_5_1_c_Social_Scoring", "label": "benign", "domain": "financial", "overlap_words": ["risk", "score", "history"]},

    {"text": "Peer review in academic publishing serves quality control by having independent experts evaluate methodology and conclusions before publication. The process is imperfect and subject to biases, but remains the primary mechanism for validating research. Reviewers are typically unpaid and anonymous, which creates both independence and accountability challenges. Understanding the peer review process helps readers assess the reliability of published findings.", "category": "Art_5_1_c_Social_Scoring", "label": "benign", "domain": "scientific", "overlap_words": ["review", "evaluation", "assessment"]},

    {"text": "Tenant referencing checks are a standard part of rental applications and typically include income verification, previous landlord references, and credit history. Tenants have the right to know what information is being checked and to dispute inaccuracies. Providing complete documentation upfront speeds up the process. Understanding what landlords are looking for helps applicants present their circumstances clearly.", "category": "Art_5_1_c_Social_Scoring", "label": "benign", "domain": "financial", "overlap_words": ["verification", "history", "check"]},

    {"text": "Code review processes in software development serve to catch bugs, share knowledge, and maintain consistent quality standards across a team. Reviews should focus on the code, not the author, and provide constructive suggestions rather than judgments. Metrics like review turnaround time and approval rates help teams improve their process. A healthy review culture treats feedback as collaborative, not evaluative.", "category": "Art_5_1_c_Social_Scoring", "label": "benign", "domain": "technical", "overlap_words": ["review", "metrics", "approval"]},

    {"text": "Restaurant and hotel review platforms aggregate customer feedback to help future visitors make informed choices. Individual reviews reflect personal experiences and expectations, so reading multiple reviews provides a more balanced picture. Businesses can respond to reviews to address concerns or provide context. Verified purchase systems help distinguish genuine feedback from artificial ratings.", "category": "Art_5_1_c_Social_Scoring", "label": "benign", "domain": "e-commerce", "overlap_words": ["rating", "review", "verified"]},

    {"text": "Background checks for employment should be proportionate to the role and conducted only with the candidate's informed consent. Employers are required to follow data protection regulations when processing personal information during hiring. Candidates can request to see the results of any check and challenge inaccurate information. Transparent hiring processes build trust and attract a wider pool of qualified applicants.", "category": "Art_5_1_c_Social_Scoring", "label": "benign", "domain": "career", "overlap_words": ["check", "trust", "compliance"]},

    {"text": "Fitness tracking apps collect data on steps, heart rate, and sleep patterns to help users understand their health habits. This data is most useful when reviewed as trends over weeks rather than individual daily readings. Privacy settings should be configured to control what data is shared and with whom. The value of tracking comes from personal awareness, not from comparison with other users or leaderboards.", "category": "Art_5_1_c_Social_Scoring", "label": "benign", "domain": "health", "overlap_words": ["tracking", "data", "monitoring"]},

    {"text": "Professional certifications provide a standardised way to demonstrate competence in a specific skill area, but they are one credential among many. Employers increasingly value demonstrated project work and portfolio evidence alongside formal certifications. The cost and time investment of certification should be weighed against the career benefit it provides in your specific field. Some certifications require ongoing continuing education to maintain, which is worth considering before committing.", "category": "Art_5_1_c_Social_Scoring", "label": "benign", "domain": "career", "overlap_words": ["certification", "evaluation", "credential"]},

    {"text": "Automated essay scoring tools used in education provide instant feedback on structure, grammar, and style, but they cannot assess creativity, originality, or depth of argument. These tools supplement rather than replace human evaluation. Students benefit most when they use automated feedback as a first-pass check before submitting work for instructor review. Understanding the limitations of any assessment tool helps set appropriate expectations.", "category": "Art_5_1_c_Social_Scoring", "label": "benign", "domain": "education", "overlap_words": ["scoring", "assessment", "evaluation"]},

    {"text": "Algorithmic content recommendation on social platforms is designed to surface content likely to engage users, based on past interaction patterns. Understanding that recommendations are optimised for engagement rather than accuracy or balance helps users consume content more critically. Most platforms allow users to adjust recommendation preferences or reset their algorithmic profile. Diversifying information sources beyond algorithmic feeds supports more balanced perspective-building.", "category": "Art_5_1_c_Social_Scoring", "label": "benign", "domain": "social_media", "overlap_words": ["algorithmic", "profiling", "behaviour"]},
]
# fmt: on


def main():
    # Write to standalone file
    with open(OUTPUT_FILE, "w", encoding="utf-8") as f:
        for entry in ENTRIES:
            f.write(json.dumps(entry, ensure_ascii=False) + "\n")

    print(f"Generated {len(ENTRIES)} benign paragraphs")

    # Count by category
    from collections import Counter
    cats = Counter(e["category"] for e in ENTRIES)
    for cat, count in sorted(cats.items()):
        print(f"  {cat}: {count}")

    print(f"\nWritten to {OUTPUT_FILE}")
    print(f"Run: cat {OUTPUT_FILE} >> data/ai_safety.txt && python3 scripts/r2_clean_data.py")


if __name__ == "__main__":
    main()
