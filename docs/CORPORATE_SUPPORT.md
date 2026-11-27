# Naso Corporate Support Program

**Enterprise-grade support for quantum computing with Naso**

---

## Overview

The Naso Corporate Support Program provides enterprise customers with:
- **Guaranteed SLAs** for critical issues
- **Dedicated engineering support** from Naso core team
- **Priority feature development** and bug fixes
- **Security audit and compliance** assistance
- **Training and onboarding** for quantum development teams
- **Long-term support (LTS)** releases

---

## Support Tiers

### Community (Free)

| Feature | Details |
|---------|---------|
| **Response Time** | Best effort (community forums) |
| **Channels** | GitHub Issues, Discord, Discourse |
| **Bug Fixes** | Next minor release |
| **Security Patches** | Best effort |
| **Documentation** | Public docs, examples |
| **Training** | Self-paced online materials |

### Professional ($2,000/month)

| Feature | Details |
|---------|---------|
| **Response Time** | 4 business hours (P1), 1 business day (P2) |
| **Channels** | Email, Slack, Video calls (monthly) |
| **Bug Fixes** | Next patch release |
| **Security Patches** | Within 72 hours |
| **Documentation** | Priority docs updates |
| **Training** | 2 half-day workshops/year |
| **Advisory** | Quarterly architecture review |

### Enterprise ($10,000/month)

| Feature | Details |
|---------|---------|
| **Response Time** | 1 hour (P1), 4 hours (P2), 1 day (P3) |
| **Channels** | Dedicated Slack, Phone, Video calls (weekly) |
| **Bug Fixes** | Hotfix within 24h (P1), next patch (P2) |
| **Security Patches** | Within 24 hours |
| **Documentation** | Custom docs, internal wiki |
| **Training** | Unlimited workshops, custom curriculum |
| **Advisory** | Monthly architecture review, roadmap input |
| **LTS** | 3-year LTS branches |
| **Compliance** | SOC2, ISO27001 assistance |

### Strategic ($50,000/month)

| Feature | Details |
|---------|---------|
| **Response Time** | 15 min (P1), 1 hour (P2), 4 hours (P3) |
| **Channels** | Dedicated TAM, 24/7 on-call, Embedded engineer |
| **Bug Fixes** | Hotfix within 4h (P1), 24h (P2) |
| **Security Patches** | Within 4 hours |
| **Custom Development** | 20% engineering capacity reserved |
| **Training** | Dedicated trainer, certification program |
| **Advisory** | Weekly sync, roadmap co-design |
| **LTS** | 5-year LTS, custom backports |
| **Compliance** | Full audit support, custom certifications |
| **IP** | Source code escrow, custom licensing |

---

## Priority Definitions

| Priority | Description | Examples |
|----------|-------------|----------|
| **P1 - Critical** | Production down, data loss, security breach | Compiler crash on valid code, quantum circuit produces wrong results, vulnerability |
| **P2 - High** | Major feature broken, performance regression | Optimization pass broken, LSP crashes, package registry down |
| **P3 - Medium** | Minor feature issue, usability problem | Diagnostic message unclear, slow compilation for large files |
| **P4 - Low** | Enhancement, documentation typo | New language feature request, typo in docs |

---

## Service Level Agreements

### Professional Tier

| Metric | Target |
|--------|--------|
| P1 Response | 4 business hours |
| P2 Response | 1 business day |
| P3 Response | 3 business days |
| P4 Response | 5 business days |
| Uptime (package registry) | 99.5% |
| Security patch (critical) | 72 hours |

### Enterprise Tier

| Metric | Target |
|--------|--------|
| P1 Response | 1 hour (24/7) |
| P2 Response | 4 hours |
| P3 Response | 1 business day |
| P4 Response | 3 business days |
| Uptime (package registry) | 99.9% |
| Security patch (critical) | 24 hours |
| LTS patch release | Monthly |

### Strategic Tier

| Metric | Target |
|--------|--------|
| P1 Response | 15 minutes (24/7) |
| P2 Response | 1 hour |
| P3 Response | 4 hours |
| P4 Response | 1 business day |
| Uptime (package registry) | 99.99% |
| Security patch (critical) | 4 hours |
| Custom feature delivery | Per agreed roadmap |

---

## Support Coverage

### Included

- **Compiler**: All backends (LLVM, Cranelift, QIR, WASM)
- **Language Server**: VS Code, IntelliJ, Neovim, Helix
- **Standard Library**: Core, quantum, crypto, async
- **Package Manager**: Registry, resolver, publisher
- **Quantum Simulator**: Statevector, stabilizer, density matrix
- **Verification**: SMT integration (Z3, CVC5)
- **Build System**: naso build, test, bench, doc

### Add-ons

| Add-on | Professional | Enterprise | Strategic |
|--------|--------------|------------|-----------|
| **Quantum Hardware Integration** | +$500/mo | Included | Included |
| **Custom Backend Development** | Quote | +$2,000/mo | Included |
| **On-premise Registry** | +$1,000/mo | Included | Included |
| **Formal Verification Audit** | Quote | +$5,000/project | 2/year included |
| **Team Certification** | $500/seat | 5 seats included | Unlimited |

---

## Onboarding Process

### Week 1: Kickoff
- [ ] Technical assessment call (2h)
- [ ] Environment setup assistance
- [ ] Access to private package registry
- [ ] Slack/email channel provisioning

### Week 2-3: Training
- [ ] Naso fundamentals workshop (4h)
- [ ] Quantum programming workshop (4h)
- [ ] Verification & formal methods workshop (4h)
- [ ] Code review of existing project

### Week 4: Production Readiness
- [ ] CI/CD pipeline setup
- [ ] Monitoring & alerting configuration
- [ ] Disaster recovery plan
- [ ] Go-live checklist

---

## Escalation Path

```
Tier 1: Support Engineer (response per SLA)
    ↓ (if unresolved in 2x SLA)
Tier 2: Senior Engineer (15 min response)
    ↓ (if unresolved in 2x SLA)
Tier 3: Core Team Lead (immediate)
    ↓ (if architectural)
Tier 4: CTO/VP Engineering (within 1 hour)
```

---

## Communication Channels

### Professional
- **Primary**: Email (support@nasolang.org)
- **Chat**: Shared Slack workspace
- **Calls**: Monthly 1h video call
- **Portal**: Private GitHub repository for issues

### Enterprise
- **Primary**: Dedicated Slack channel
- **Phone**: Direct line to support engineer
- **Calls**: Weekly 30min sync
- **Portal**: Private GitHub + Jira integration
- **On-site**: 2 days/quarter (travel extra)

### Strategic
- **Primary**: Dedicated TAM (Technical Account Manager)
- **Embedded**: 1 engineer 20% time on-site/remote
- **Calls**: Weekly 1h + ad-hoc
- **Portal**: Full toolchain integration
- **On-site**: Monthly 2-day visits included

---

## Security & Compliance

### Vulnerability Reporting

```
security@nasolang.org
```

- Acknowledgment within 4 hours (Enterprise/Strategic)
- Triage within 24 hours
- Patch timeline communicated within 48 hours
- Coordinated disclosure (90 days default)

### Compliance Support

| Standard | Professional | Enterprise | Strategic |
|----------|--------------|------------|-----------|
| **SOC2 Type II** | Docs only | Evidence package | Audit participation |
| **ISO 27001** | Docs only | Evidence package | Audit participation |
| **FIPS 140-2** | N/A | Guidance | Full support |
| **Common Criteria** | N/A | N/A | Full support |
| **Export Control (EAR/ITAR)** | Guidance | Classification help | Full compliance |

### Data Handling

- **Customer code**: Never stored, processed in-memory only
- **Telemetry**: Opt-in, anonymized, aggregated
- **Logs**: Retained 30 days, encrypted at rest
- **Backups**: Encrypted, geo-replicated, 90-day retention

---

## Billing & Contracts

### Payment Terms

- **Monthly**: Net 30, credit card/wire
- **Annual**: 10% discount, net 30, wire transfer
- **Multi-year**: 15-20% discount, custom terms

### Contract Length

| Tier | Minimum | Renewal |
|------|---------|---------|
| Professional | 3 months | Auto-renew monthly |
| Enterprise | 12 months | Auto-renew annually |
| Strategic | 24 months | Custom |

### SLA Credits

| Tier | Monthly Uptime < Target | Credit |
|------|------------------------|--------|
| Professional | 99.5% | 10% |
| Enterprise | 99.9% | 15% |
| Strategic | 99.99% | 25% |

---

## Customer Success Program

### Quarterly Business Reviews (QBR)

- **Enterprise**: Quarterly
- **Strategic**: Monthly

### Metrics Tracked

- Compiler adoption rate
- Build time trends
- Bug density
- Quantum circuit success rate
- Team productivity (PR velocity)
- Training completion

### Health Scores

| Score | Action |
|-------|--------|
| 90-100 | Green - Continue current plan |
| 70-89 | Yellow - Proactive outreach |
| 50-69 | Orange - Action plan required |
| <50 | Red - Executive escalation |

---

## Reference Customers

> "Naso's corporate support enabled our team to ship quantum-enhanced ML models 6 months ahead of schedule. The dedicated engineer became part of our team."
> — **QuantumML Inc**, Strategic Tier

> "The 24/7 support and LTS releases give us confidence to run Naso in production for our financial risk models."
> — **FinQuant Capital**, Enterprise Tier

> "Migration from Rust was seamless with cargo2naso and the Professional support team guided us through every step."
> — **TechStart Labs**, Professional Tier

---

## Getting Started

### Contact

```
sales@nasolang.org
+1 (555) NASO-LANG
https://nasolang.org/enterprise
```

### Evaluation

1. **Discovery call** (30 min) - Understand requirements
2. **Technical trial** (2 weeks) - Full Enterprise access
3. **Proposal** - Customized tier recommendation
4. **Contract** - MSA + SLA
5. **Kickoff** - Within 5 business days

---

## FAQ

### Can I upgrade/downgrade tiers?
Yes, with 30 days notice. Upgrades immediate, downgrades at renewal.

### What's included in "custom development"?
Features on the public roadmap prioritized for you. Custom backends, language extensions, tooling.

### Is there a trial?
Enterprise tier includes 2-week full-featured trial.

### Do you support air-gapped environments?
Yes, Enterprise+ includes on-premise registry and offline toolchain bundles.

### What about IP ownership?
Customer retains all IP. Strategic tier includes source code escrow.

### How are quantum hardware costs handled?
Hardware access (IBM, Rigetti, IonQ) billed separately. Naso provides integration layer.

---

*Version 1.0 | Effective 2024 | Subject to MSA*